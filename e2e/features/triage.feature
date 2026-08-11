@parallel
Feature: Triage entries (star, mark-read, summarize)

  Background:
    Given I am signed in
    And I have a feed "Triage Feed" with 3 test entries in category "Triage Category"

  Scenario: Starring an entry updates the row and the sidebar starred count
    When I open the inbox
    And I star the entry titled "Test Entry 1"
    Then the entry titled "Test Entry 1" is marked starred
    And the sidebar starred count is at least 1

  Scenario: Marking an entry read updates the row and the sidebar unread count
    When I open the inbox
    And I mark the entry titled "Test Entry 1" read
    Then the entry row for "Test Entry 1" shows as read
    And the sidebar unread count decreases by 1

  Scenario: The row read-dot toggles the entry between read and unread
    When I open the inbox
    And I click the read toggle for the entry titled "Test Entry 1"
    Then the entry row for "Test Entry 1" shows as read
    When I click the read toggle for the entry titled "Test Entry 1"
    Then the entry row for "Test Entry 1" shows as unread

  # Regression guard: the 0.55.0 redesign silently dropped the per-row
  # mark-read control and the open-original link. These assertions fail loudly
  # if a future UI change removes any per-row control again.
  Scenario: Every entry row keeps its full set of per-row controls
    When I open the inbox
    Then every entry row exposes the read toggle, star, open-original, time, and feed controls
    And every open-original link points at the entry's source URL

  Scenario: The entry title highlights on hover to signal it is clickable
    When I open the inbox
    Then the entry title for "Test Entry 1" highlights on hover

  Scenario: Marking all entries read empties the unread list
    When I open the inbox
    And I mark the document for reload detection
    And I mark all entries as read
    Then I see 0 entries in the entry list
    And the document did not reload

  # The age options are the dropdown path that leaves rows behind, so a
  # regression to `location.reload()` costs the reader their scroll position
  # and open entry for a list that only shrank by one row.
  Scenario: Marking entries older than 1 day as read swaps the list in place
    Given the feed "Triage Feed" has an entry titled "Ancient News" published 3 days ago
    When I open the inbox
    And I mark the document for reload detection
    And I mark entries older than 1 day as read
    Then the entry list does not show "Ancient News"
    And I see 3 entries in the entry list
    And the document did not reload

  Scenario: Summarizing an entry shows the summary in the reading pane
    Given the user has Kagi configured
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Summarize" button
    Then the reading pane shows a summary
    And the reading-pane summarize toggle still shows its icon
    And the reading-pane summarize toggle reads "Dismiss"

  Scenario: A summary that lands after the reader moved on stays out of the new entry
    Given the user has Kagi configured
    When I open the inbox
    And the summary fragment response is held
    And I click the entry titled "Test Entry 1"
    And I click the "Summarize" button
    And the summary fragment request is in flight
    And I click the entry titled "Test Entry 2"
    And the held summary fragment response lands
    Then the reading pane shows the title "Test Entry 2"
    And the reading pane shows no summary

  Scenario: Dismissing a summary clears the summary from the reading pane
    Given the entry titled "Test Entry 1" has a summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Dismiss" summary action
    Then the reading pane summary is dismissed

  Scenario: The action-bar toggle shows Dismiss and dismisses an existing summary
    Given the user has Kagi configured
    And the entry titled "Test Entry 1" has a summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading-pane summarize toggle reads "Dismiss"
    When I click the reading-pane summarize toggle
    Then the reading pane summary is dismissed
    And the reading-pane summarize toggle reads "Summarize"

  Scenario: The summarize toggle is inert while a summary is in flight
    Given the user has Kagi configured
    And the entry titled "Test Entry 1" has a pending summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading-pane summarize toggle reads "Summarize"
    And the reading-pane summarize toggle is disabled
    When I watch for summarize POST requests
    And I press the "a" key
    Then no summarize POST request is sent

  Scenario: a starts summarization from the keyboard
    Given the user has Kagi configured
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "a" key
    Then the reading pane shows a summary

  Scenario: a dismisses an existing summary
    Given the entry titled "Test Entry 1" has a summary
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I press the "a" key
    Then the reading pane summary is dismissed

  Scenario: Mark Above as Read refreshes the list without reloading the page
    When I open the inbox
    And I mark the document for reload detection
    And I mark the loaded entries as read
    Then the entry list does not show "Test Entry 1"
    And the entry list does not show "Test Entry 3"
    And the document did not reload

  Scenario: Mark Above as Read leaves the open entry in the reading pane
    When I open the entries page for category "Triage Category"
    And I click the entry titled "Test Entry 1"
    And I mark the loaded entries as read
    Then the reading pane shows the title "Test Entry 1"
    And the entry list does not show "Test Entry 3"

  # Both refresh paths re-render the whole row container with genuinely changed
  # markup — the rows survive and only gain `entry-read` — so the
  # skip-when-unchanged guard cannot apply. Replacing the container rebuilt
  # every row and every favicon in it (measured: none of six preserved), and a
  # rebuilt <img> is what WebKit blinks. The container is morphed instead.
  Scenario: Mark Above as Read keeps the rows and favicons already on screen
    Given the "Triage Feed" feed has a favicon
    When I open the entries page for category "Triage Category" showing all statuses
    And the entry list favicons have loaded
    And I tag the entry list contents
    And I mark the loaded entries as read
    Then every entry in the list is marked read
    And the entry list contents are still the ones I tagged

  # The refresh answers with page 1 again, so an offset kept from before the
  # mark points at rows the reader has already triaged — and on an unread list
  # it points past the end of everything that just disappeared. `status=all`
  # keeps the rows (and therefore the scroll extent), which is what makes the
  # offset observable at all.
  Scenario: Mark Above as Read returns the list to the top
    Given I have a feed "Scroll Feed" with 30 test entries in category "Triage Category"
    When I open the entries page for category "Triage Category" showing all statuses
    And I scroll the entry list to the bottom
    And I mark the loaded entries as read
    Then every entry in the list is marked read
    And the entry list is scrolled to the top

  Scenario: The Mark as Read dropdown keeps the rows and favicons already on screen
    Given the "Triage Feed" feed has a favicon
    When I open the entries page for category "Triage Category" showing all statuses
    And the entry list favicons have loaded
    And I tag the entry list contents
    And I mark all entries as read
    Then every entry in the list is marked read
    And the entry list contents are still the ones I tagged

  Scenario: Scoped search within a category, then mark matching as read
    Given a category "Anime" containing entries titled "Superheroine Rises" and "Other News"
    When I open the entries page for category "Anime"
    And I open the scoped search box
    And I type "Superheroine" into the scoped search box
    Then the entry list shows "Superheroine Rises"
    And the entry list does not show "Other News"
    When I mark matching entries as read
    Then "Superheroine Rises" is no longer in the unread list

  Scenario: The scoped search box starts collapsed and opens from the filter bar
    Given a category "Anime" containing entries titled "Superheroine Rises" and "Other News"
    When I open the entries page for category "Anime"
    Then the scoped search box is closed
    And the mark-above button is shown
    And the search toggle is as tall as the status filter
    When I open the scoped search box
    Then the scoped search box is open
    And the search close button is as tall as the search box

  Scenario: Closing the scoped search box clears the search
    Given a category "Anime" containing entries titled "Superheroine Rises" and "Other News"
    When I open the entries page for category "Anime"
    And I open the scoped search box
    And I type "Superheroine" into the scoped search box
    Then the entry list does not show "Other News"
    And the mark-above button is hidden
    When I close the scoped search box
    Then the scoped search box is closed
    And the entry list shows "Other News"
    And the URL has no "q" query parameter

  Scenario: A scoped-search deep link arrives with the search box open
    Given a category "Anime" containing entries titled "Superheroine Rises" and "Other News"
    When I open the entries page for category "Anime" searching for "Superheroine"
    Then the scoped search box is open
    And the entry list does not show "Other News"
    And the mark-above button is hidden

  Scenario: Clearing the scoped search box resets the q query parameter
    Given a category "Anime" containing entries titled "Superheroine Rises" and "Other News"
    When I open the entries page for category "Anime"
    And I open the scoped search box
    And I type "Superheroine" into the scoped search box
    Then the entry list shows "Superheroine Rises"
    And the entry list does not show "Other News"
    And the URL has the "q" query parameter set to "Superheroine"
    When I clear the scoped search box
    Then the entry list shows "Other News"
    And the URL has no "q" query parameter
