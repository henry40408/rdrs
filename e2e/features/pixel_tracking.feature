@parallel
Feature: Open tracking

  # Ordering is load-bearing throughout: the opt-in timestamp is the baseline
  # the open rate is measured from, so entries seeded before it carry no pixel
  # and are outside the denominator. Every scenario that expects a rate turns
  # tracking on *first*.

  Background:
    Given I am signed in

  Scenario: The open rate column stays out of the way until I opt in
    Given I have a feed "Quiet Feed" with 5 test entries in category "Tracking"
    When I am on the feeds page
    Then the feeds table has no open rate column
    When I turn on open tracking
    And I am on the feeds page
    Then the feeds table has an open rate column

  Scenario: Reading an entry in the browser is what the open rate counts
    Given I have open tracking turned on
    And I have a feed "Tracked Feed" with 5 test entries in category "Tracking"
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane carries a tracking pixel
    When I am on the feeds page
    Then the open rate for "Tracked Feed" is "20% (1/5)"

  Scenario: Nothing is added to my entries while I am opted out
    Given I have a feed "Untracked Feed" with 5 test entries in category "Tracking"
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane carries no tracking pixel

  # The sibling of "Saving the queue does not mark the queue read" in
  # offline_reading.feature: mirroring the queue fetches the images an entry
  # references so the article is readable without a connection, and the pixel is
  # an image. Fetching it would report an open for every entry the reader has
  # merely queued.
  Scenario: Mirroring entries for offline reading is not an open
    Given I have open tracking turned on
    And I have a feed "Mirrored Feed" with 5 test entries in category "Tracking"
    And I keep 10 entries for offline reading
    And a service worker controls the page
    And 5 entries have been saved for offline reading
    When I am on the feeds page
    Then the open rate for "Mirrored Feed" is "0% (0/5)"

  Scenario: A feed with too little data reports nothing rather than a bad number
    Given I have open tracking turned on
    And I have a feed "Sparse Feed" with 2 test entries in category "Tracking"
    When I am on the feeds page
    Then the open rate for "Sparse Feed" is not reported yet

  Scenario: Statistics puts the least-opened feed at the top
    Given I have open tracking turned on
    And I have a feed "Tracked Feed" with 5 test entries in category "Tracking"
    And every entry in "Tracked Feed" has been opened
    And I have a feed "Ignored Feed" with 5 test entries in category "Tracking"
    When I open the statistics page
    Then the statistics page ranks "Ignored Feed" above "Tracked Feed" by open rate
