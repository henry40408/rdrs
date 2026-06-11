@parallel
Feature: Category navigation shortcuts

  Background:
    Given I am signed in
    And the default "Uncategorized" category is removed
    And I have a feed "Alpha Feed" with 2 test entries in category "Cat A"
    And I have a feed "Bravo Feed" with 2 test entries in category "Cat B"
    And I have a feed "Charlie Feed" with 2 test entries in category "Cat C"

  Scenario: ] jumps from the current category to the next in sidebar order
    When I open the entries page for category "Cat B"
    And I press the "]" key
    Then I am on the entries page for category "Cat C"

  Scenario: [ jumps from the current category to the previous in sidebar order
    When I open the entries page for category "Cat B"
    And I press the "[" key
    Then I am on the entries page for category "Cat A"

  Scenario: ] wraps from the last category back to the first
    When I open the entries page for category "Cat C"
    And I press the "]" key
    Then I am on the entries page for category "Cat A"

  Scenario: [ wraps from the first category to the last
    When I open the entries page for category "Cat A"
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
    And I press the "]" key
    Then I am on the entries page for category "Cat A"

  Scenario: [ from the unread inbox enters the last category
    When I open the inbox
    And I press the "[" key
    Then I am on the entries page for category "Cat C"

  Scenario: ] on a feed page continues from the feed's parent category
    When I open the entries page for feed "Alpha Feed"
    And I press the "]" key
    Then I am on the entries page for category "Cat B"
