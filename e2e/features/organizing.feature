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

  Scenario: Creating a category via the form adds it to the categories table
    Given I am on the categories page
    When I create a category named "Tech News"
    Then I see a success flash "Category created."
    And the categories table contains "Tech News"

  Scenario: Renaming a category inline persists the new name
    Given I am on the categories page
    When I rename category "My Category" to "Renamed Category"
    Then I see a success flash "Category renamed."
    And the categories table contains "Renamed Category"

  Scenario: Deleting a category removes it from the categories table
    Given I am on the categories page
    When I confirm the next dialog
    And I delete category "My Category"
    Then I see a success flash "Category deleted."
    And the categories table does not contain "My Category"

  Scenario: Filtering feeds by category narrows the visible rows
    Given I have a category named "Other Category"
    And I have a feed "Cat A Feed" in category "My Category"
    And I have a feed "Cat B Feed" in category "Other Category"
    And I am on the feeds page
    When I filter feeds by category "Other Category"
    Then the feeds table contains "Cat B Feed"
    And the feeds table does not contain "Cat A Feed"

  Scenario: Refreshing a feed shows a success flash
    Given I have a feed from the mock RSS server in category "My Category"
    And I am on the feeds page
    When I refresh the feed "Test Feed"
    Then I see a success flash "Refreshed:"
