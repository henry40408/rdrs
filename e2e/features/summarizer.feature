@summarizer
Feature: Summarizer tool
  As a logged-in user with Kagi configured
  I can summarize several URLs at once without adding them to a feed

  Scenario: Summaries resolve in order
    Given I am signed in
    And the user has Kagi configured
    When I open the Summarizer
    And I enter these URLs:
      | https://example.com/one |
      | https://example.com/two |
    And I submit the summarizer form
    Then I should see 2 summary cards
    And each card resolves to a completed state containing "E2E mock summary body."

  Scenario: Settings prompt when Kagi is not configured
    Given I am signed in
    When I open the Summarizer
    Then I should see a link to Settings
    And I should not see the summarizer form
