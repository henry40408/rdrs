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
    And the actions on a feed row line up on one axis

  Scenario: Creating a category via the form adds it to the categories table
    Given I am on the categories page
    When I create a category named "Tech News"
    Then I see a success flash "Category created."
    And the flash banner shows a timestamp
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

  Scenario: Editing a feed updates its title in the feeds table
    Given I have a feed "Old Feed" in category "My Category"
    And I am on the feeds page
    When I edit the feed "Old Feed" and set its title to "New Feed Name"
    Then I see a success flash "Feed updated."
    When I am on the feeds page
    Then the feeds table contains "New Feed Name"

  Scenario: Deleting a feed removes it from the feeds table
    Given I have a feed "Doomed Feed" in category "My Category"
    And I am on the feeds page
    When I confirm the next dialog
    And I delete the feed "Doomed Feed"
    Then I see a success flash "Feed deleted."
    And the feeds table does not contain "Doomed Feed"

  Scenario: Importing an OPML file adds its feeds to the feeds table
    Given I am on the import OPML page
    When I import the OPML fixture "sample.opml"
    Then I see a success flash "OPML imported: 1 feed added."
    And the feeds table contains "Imported Feed"

  Scenario: Exporting OPML includes my subscribed feeds
    Given I have a feed "Exported Feed" in category "My Category"
    Then the exported OPML contains "Exported Feed"
