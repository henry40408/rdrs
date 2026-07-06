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

  Scenario: A highlighted Latin term is not split mid-word on narrow screens
    Given I have an entry titled "X 推托管版 MCP：AI agents 免設定直連　Grok 即用 X API、立刻使用即時資訊源"
    And I use a narrow phone viewport
    And I am on the search page
    When I search for "Grok"
    Then the highlighted term "Grok" renders on a single line
    And the highlighted title flows as one inline block
