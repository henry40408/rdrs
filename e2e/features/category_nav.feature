@parallel
Feature: Category navigation

  Background:
    Given I am signed in
    And the default "Uncategorized" category is removed
    And I have a feed "Alpha Feed" with 2 test entries in category "Cat A"
    And I have a feed "Bravo Feed" with 2 test entries in category "Cat B"
    And I have a feed "Charlie Feed" with 2 test entries in category "Cat C"

  Scenario: Clicking a sidebar category swaps the list in place
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    And I mark the document for reload detection
    And I click the sidebar category "Cat B"
    Then I am on the entries page for category "Cat B"
    And the document did not reload
    And the list header shows "Cat B"
    And I see 2 entries in the entry list
    And the sidebar highlights category "Cat B"

  Scenario: Switching category keeps the sidebar scrolled where it was
    Given I have 40 more categories
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    And I scroll the sidebar categories to the bottom
    And I click the last sidebar category
    Then the sidebar is still scrolled where it was

  Scenario: Switching category closes the entry left open
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    And I click the entry titled "Test Entry 1"
    And I click the sidebar category "Cat B"
    Then I am on the entries page for category "Cat B"
    And the reading pane is empty
    And the URL has no ?entry= parameter

  Scenario: Going back returns to the category left behind
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    And I click the sidebar category "Cat B"
    And I am on the entries page for category "Cat B"
    And I go back in the browser
    Then I am on the entries page for category "Cat A"
    And the list header shows "Cat A"
    And the sidebar highlights category "Cat A"

  Scenario: ] jumps from the current category to the next in sidebar order
    When I open the entries page for category "Cat B"
    And the sidebar has loaded its categories
    And I press the "]" key
    Then I am on the entries page for category "Cat C"

  Scenario: [ jumps from the current category to the previous in sidebar order
    When I open the entries page for category "Cat B"
    And the sidebar has loaded its categories
    And I press the "[" key
    Then I am on the entries page for category "Cat A"

  Scenario: ] wraps from the last category back to the first
    When I open the entries page for category "Cat C"
    And the sidebar has loaded its categories
    And I press the "]" key
    Then I am on the entries page for category "Cat A"

  Scenario: [ wraps from the first category to the last
    When I open the entries page for category "Cat A"
    And the sidebar has loaded its categories
    And I press the "[" key
    Then I am on the entries page for category "Cat C"

  Scenario: Shift+] skips categories that have no unread entries
    Given all entries in category "Cat B" are marked read
    When I open the entries page for category "Cat A"
    And the sidebar shows no unread for category "Cat B"
    And I press the "}" key
    Then I am on the entries page for category "Cat C"

  Scenario: ] from the unread inbox enters the first category
    When I open the inbox
    And the sidebar has loaded its categories
    And I press the "]" key
    Then I am on the entries page for category "Cat A"

  Scenario: [ from the unread inbox enters the last category
    When I open the inbox
    And the sidebar has loaded its categories
    And I press the "[" key
    Then I am on the entries page for category "Cat C"

  Scenario: ] on a feed page continues from the feed's parent category
    When I open the entries page for feed "Alpha Feed"
    And the sidebar has loaded its categories
    And I press the "]" key
    Then I am on the entries page for category "Cat B"
