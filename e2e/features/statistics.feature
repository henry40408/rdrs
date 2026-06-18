Feature: Daily Read Articles chart

  Background:
    Given I am signed in
    And I have read articles over several days

  Scenario: Tapping a bar reveals that day's count
    When I open the statistics page
    And I tap the tallest read-activity bar
    Then the chart info card shows a read count
