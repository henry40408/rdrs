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

  Scenario: Typing searches without pressing Enter, and clearing resets
    When I type "Rust" into the search box
    Then the result count is 2
    And the URL has the "q" query parameter set to "Rust"
    When I clear the search box
    Then I see the search prompt
    And the URL has no "q" query parameter

  Scenario: Load More appends the next page of results
    Given I have 55 entries titled "Wombat"
    And I am on the search page
    When I search for "Wombat"
    Then the result count is 50
    When I load more search results
    Then the result count is 55

  Scenario: Pressing the slash key focuses the search input
    When I press the "/" key
    Then the search input is focused

  Scenario: Searching for a term with no matches shows an empty state
    When I search for "Kotlin"
    Then I see the empty-results message

  Scenario: A highlighted Latin term is not split mid-word on narrow screens
    Given I have an entry titled "X 推托管版 MCP：AI agents 免設定直連　Grok 即用 X API、立刻使用即時資訊源"
    And I use a narrow phone viewport
    And I am on the search page
    When I search for "Grok"
    Then the highlighted term "Grok" renders on a single line
    And the highlighted title flows as one inline block
