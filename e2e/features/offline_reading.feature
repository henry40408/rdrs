@parallel
Feature: Offline reading

  rdrs renders on the server, so reading without a connection means the browser
  has to be holding the articles already. It only ever does that because the
  reader asked: `offline_keep` is zero by default, and at zero this feature
  costs a reader nothing and stores nothing of theirs.

  What is stored is the server's own reading-pane markup, under a cache named
  after an opaque per-user key, and the mirroring is done with `?offline=1` so
  that opening every entry in the queue does not mark the queue read.

  Background:
    Given I am signed in
    And I have a feed "Offline Feed" with 3 test entries in category "Offline Category"

  Scenario: Nothing is stored until the reader turns it on
    Given a service worker controls the page
    Then the worker's cache holds nothing but public assets

  Scenario: The saved entries are readable with the network gone
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When the network goes offline
    And I visit "/entries/offline"
    Then I see 3 entries in the entry list
    When I click the entry titled "Test Entry 1"
    Then the reading pane shows the title "Test Entry 1"
    And the reading pane shows the content "Content for test entry 1"

  Scenario: Saving the queue does not mark the queue read
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When I open the inbox
    Then I see 3 entries in the entry list

  Scenario: A dead navigation lands on the saved entries rather than the apology
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When the network goes offline
    And I visit "/"
    Then I see 3 entries in the entry list

  Scenario: Nothing offline throws the reader off the list they still have
    Given I have a feed "Long Feed" with 51 test entries in category "Long Category"
    When I open the inbox
    And the network goes offline
    Then Load More is disabled
    When I open the first entry in the list
    Then I am told the action has to wait for the connection
    And I see 50 entries in the entry list

  Scenario: Everything that needs the server is visibly out of reach
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When the network goes offline
    And I visit "/entries/offline"
    Then every control that needs the server is disabled
    And opening a saved entry is still offered

  Scenario: A shortcut that reaches the server says so rather than failing
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When the network goes offline
    And I visit "/entries/offline"
    And I click the entry titled "Test Entry 1"
    And I press the "m" key
    Then I am told the action has to wait for the connection

  Scenario: A comment that describes an import does not become a request
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    Then nothing has been asked of the server that names no file

  Scenario: Turning offline reading off throws the saved entries away
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When I stop keeping entries for offline reading
    Then the worker's cache holds nothing but public assets
