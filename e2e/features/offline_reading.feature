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

  # The order here is the design, not convenience. A page loaded while online
  # has no way to learn the connection died: `navigator.onLine` reports having
  # an interface rather than anything answering on it, and browsers disagree on
  # whether they even fire the event — Chrome on CI does not under network
  # emulation, where the author's did. So the first failed request is what
  # reveals it, and that request must therefore not be the thing that goes
  # wrong: it keeps the reader on their list and says why, and from then on
  # everything that needs the server is out of reach.
  Scenario: The first thing to fail offline explains itself and disables the rest
    Given I have a feed "Long Feed" with 51 test entries in category "Long Category"
    When I open the inbox
    And the network goes offline
    And I open the first entry in the list
    Then I am told the action has to wait for the connection
    And I see 30 entries in the entry list
    And Load More is disabled

  # A connection is a state, not an event. It used to be announced with a flash
  # banner, which meant a message over the list every time one blinked and
  # nothing at all once it was dismissed. The lamp is CSS reacting to
  # `<html data-offline>`, so it is still right after the sidebar rebuilds its
  # own markup — which it does on every mark-as-read.
  Scenario: The sidebar lamp is what reports the connection
    When I open the inbox
    Then the sidebar shows the connection is up
    When the network goes offline
    And I open the first entry in the list
    Then the sidebar shows the connection is gone

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

  # A list page holds one page of rows and Load More reaches the server, so with
  # the connection gone everything past the first page is out of reach — however
  # much of it the browser is actually holding. The library is where all of it
  # is, and the sidebar is the only way in.
  Scenario: Everything saved is one click away when Load More cannot help
    Given I have a feed "Long Feed" with 51 test entries in category "Long Category"
    And I keep 200 entries for offline reading
    And a service worker controls the page
    And 54 entries have been saved for offline reading
    When I open the inbox
    And the network goes offline
    And I open the first entry in the list
    Then Load More is disabled
    And I see 30 entries in the entry list
    When I open the saved entries from the sidebar
    Then I see 54 entries in the entry list

  Scenario: Both navigations lead to the library once entries are being saved
    Given I keep 10 entries for offline reading
    When I open the inbox
    Then the sidebar offers the saved entries
    And the scriptless navigation offers the saved entries

  Scenario: Neither offers it while nothing is being saved
    When I open the inbox
    Then the sidebar does not offer the saved entries
    And the scriptless navigation does not offer the saved entries

  Scenario: Turning offline reading off throws the saved entries away
    Given I keep 10 entries for offline reading
    And a service worker controls the page
    And my entries have been saved for offline reading
    When I stop keeping entries for offline reading
    Then the worker's cache holds nothing but public assets
