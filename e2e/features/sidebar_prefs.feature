@parallel
Feature: Sidebar display preferences

  Background:
    Given I am signed in
    And the default "Uncategorized" category is removed
    And I have a feed "Alpha Feed" with 1 test entries in category "Cat A"
    And I have a feed "Bravo Feed" with 3 test entries in category "Cat B"
    And I have a feed "Zulu Feed" with 2 test entries in category "Cat B"

  Scenario: Categories are listed by name by default
    When I open the inbox
    And the sidebar has loaded its categories
    Then the sidebar categories read "Cat A, Cat B"

  Scenario: The unread ordering puts the busiest category first
    When I set the sidebar order to "unread"
    And I open the inbox
    And the sidebar has loaded its categories
    Then the sidebar categories read "Cat B, Cat A"

  Scenario: The unread ordering also reorders the open category's feeds
    When I set the sidebar order to "unread"
    And I open the entries page for category "Cat B"
    And the sidebar lists feed "Bravo Feed"
    Then the sidebar feeds read "Bravo Feed, Zulu Feed"

  Scenario: Hiding fully-read groups drops a read category from the list
    Given all entries in category "Cat A" are marked read
    And fully-read categories and feeds are hidden
    When I open the inbox
    And the sidebar has loaded its categories
    Then the sidebar categories read "Cat B"

  Scenario: The category being read stays listed even with nothing unread left
    Given all entries in category "Cat A" are marked read
    And fully-read categories and feeds are hidden
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    Then the sidebar categories read "Cat A, Cat B"
    And the sidebar leaves no gap below category "Cat A"

  Scenario: Hiding fully-read groups drops a read feed from the open category
    Given all entries in feed "Zulu Feed" are marked read
    And fully-read categories and feeds are hidden
    When I open the entries page for category "Cat B"
    And the sidebar lists feed "Bravo Feed"
    Then the sidebar feeds read "Bravo Feed"

  Scenario: Both settings survive a round trip through the settings page
    When I set the sidebar order to "unread"
    And fully-read categories and feeds are hidden
    Then the sidebar order field shows "unread"
    And the hide-fully-read checkbox is checked
