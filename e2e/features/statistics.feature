Feature: Daily Read Articles chart

  Background:
    Given I am signed in
    And I have read articles over several days

  Scenario: Tapping a bar reveals that day's count
    When I open the statistics page
    And I tap the tallest read-activity bar
    Then the chart info card shows a read count

  Scenario: The info card appears just above the tapped bar
    When I open the statistics page
    And I tap the single-read bar
    Then the info card sits just above that bar
