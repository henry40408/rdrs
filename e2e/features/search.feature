@parallel
Feature: Search

  Background:
    Given I am signed in
    And I have a feed with entries titled:
      | Rust Programming Guide |
      | JavaScript Frameworks  |
      | Rust Async Runtime     |
    And I am on the search page

  Scenario: Searching for a term shows matching entries
    When I search for "Rust"
    Then I see search results:
      | Rust Programming Guide |
      | Rust Async Runtime     |
    And the result count is 2

  Scenario: Pressing the slash key focuses the search input
    When I press the "/" key
    Then the search input is focused

  Scenario: Searching for a term with no matches shows an empty state
    When I search for "Kotlin"
    Then I see the empty-results message
