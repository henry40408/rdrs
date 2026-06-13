@parallel
Feature: Responsive layout

  Background:
    Given I am signed in

  @mobile
  Scenario: Sidebar is hidden by default and toggled by the hamburger on mobile
    Given I am viewing on a mobile screen
    When I open the inbox
    Then the sidebar is not visible
    When I tap the hamburger
    Then the sidebar is visible
    When I tap the sidebar close button
    Then the sidebar is not visible

  @mobile
  Scenario: Tapping outside the open sidebar drawer closes it on mobile
    Given I am viewing on a mobile screen
    When I open the inbox
    And I tap the hamburger
    Then the sidebar is visible
    When I tap outside the sidebar
    Then the sidebar is not visible

  @mobile
  Scenario: Entry list is full-width single column on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is at least 370px wide

  @mobile
  Scenario: Opening an entry on mobile reveals the reading pane as a full-screen overlay
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    Then the reading pane is visible on mobile

  @mobile
  Scenario: Tapping back on the reading pane on mobile returns to the entry list
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    And I click the entry titled "Test Entry 1"
    And I tap the reading-pane back button
    Then the reading pane is empty
    And the URL has no ?entry= parameter

  @mobile
  Scenario: Deep-linking with ?entry= on mobile shows the reading pane
    Given I am viewing on a mobile screen
    And I have a feed "Reading Feed" with 5 test entries in category "Reading Category"
    When I open the inbox deep-linked to entry titled "Test Entry 2"
    Then the reading pane is visible on mobile

  @mobile
  Scenario: Categories table renders as cards on mobile
    Given I am viewing on a mobile screen
    And I have a category named "Test Category"
    When I open the categories page
    Then the categories table is shown as cards

  @mobile
  Scenario: Flash banner clears the hamburger on mobile
    Given I am viewing on a mobile screen
    When I open the inbox
    And a flash banner is shown
    Then the flash banner sits below the hamburger
    And the ".banner-dismiss" control is at least 44px wide
    And the ".banner-dismiss" control is at least 44px tall

  @tablet
  Scenario: Sidebar is a drawer on tablet
    Given I am viewing on a tablet screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the sidebar is not visible
    And the hamburger button is visible
    When I tap the hamburger
    Then the sidebar is visible

  @tablet
  Scenario: Tables keep table layout on tablet
    Given I am viewing on a tablet screen
    And I have a category named "Test Category"
    When I open the categories page
    Then the categories table is shown as a table

  @tablet
  Scenario: Entry list is full-width single column on tablet
    Given I am viewing on a tablet screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is at least 760px wide

  @desktop
  Scenario: Sidebar is always visible on desktop
    Given I am viewing on a desktop screen
    When I open the inbox
    Then the sidebar is always-visible

  @desktop
  Scenario: Reading pane sits beside the entry list on desktop
    Given I am viewing on a desktop screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the entry list pane is narrower than the viewport

  @mobile
  Scenario: Inbox and drawer controls meet the 44px touch minimum on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the inbox
    Then the ".sidebar-toggle" control is at least 44px wide
    And the ".sidebar-toggle" control is at least 44px tall
    And the ".entry-action-btn" control is at least 44px tall
    And the ".filter-bar select" control is at least 44px tall
    When I tap the hamburger
    Then the ".sidebar-close" control is at least 44px wide
    And the ".sidebar-close" control is at least 44px tall
    And the ".sidebar-item" control is at least 44px tall
    When I tap the sidebar close button
    And I click the entry titled "Test Entry 1"
    Then the reading pane is visible on mobile
    And the ".reading-pane-back-link" control is at least 44px tall

  @mobile
  Scenario: Table action links meet the 44px touch minimum on mobile
    Given I am viewing on a mobile screen
    And I have a category named "Test Category"
    When I open the categories page
    Then the ".action-link" control is at least 44px tall
    And the "#name" control is at least 44px tall

  @mobile
  Scenario: Tab bar links meet the 44px touch minimum on mobile
    Given I am viewing on a mobile screen
    And I have a feed with 5 test entries
    When I open the all-entries page
    Then the ".tab-bar a" control is at least 44px tall
