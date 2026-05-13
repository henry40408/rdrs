@parallel
Feature: Organizing feeds and categories

  Background:
    Given I am signed in
    And I have a category named "My Category"

  Scenario: Adding a feed makes it appear in the feeds table
    Given I am on the feeds page
    When I add a feed from the mock RSS server under "My Category"
    Then I see a success flash "Feed added"
    And the feeds table contains "Test Feed"
