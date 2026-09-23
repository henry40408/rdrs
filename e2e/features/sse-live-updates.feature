@parallel
Feature: Live updates via SSE

  Background:
    Given I am signed in
    And I have a feed "SSE Feed" with 5 test entries in category "SSE Category"

  # An external POST /entries/{id}/read emits an SSE sidebar event, which must
  # refetch /api/sidebar and drop the count without a reload.
  Scenario: Sidebar unread count updates live without a page reload
    When I open the inbox
    And a background request marks "Test Entry 1" as read
    Then within 5 seconds the sidebar unread count decreases by one without a reload

  Scenario: Summary completes and the reading pane and row badge update live
    Given the user has Kagi configured
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I click the "Summarize" button
    Then the entry row shows a pending summary badge
    And without reloading, the reading pane shows the completed summary
    And the entry row shows the completed summary badge
