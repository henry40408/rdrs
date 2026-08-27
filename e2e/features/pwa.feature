@parallel
Feature: Installable app and offline fallback

  rdrs renders on the server, so there is genuinely nothing to read without a
  connection. What a service worker can add is that the app says so in its own
  voice instead of handing the reader the browser's error page, and that it
  keeps being installable — a manifest alone does not earn an install prompt.

  The worker is registered from the signed-in layout, and precaches exactly two
  things: the offline page and the stylesheet it needs. Nothing that belongs to
  a reader may ever reach that cache, because every signed-in response is
  `no-store` and the Cache API honours no such header.

  Scenario: A service worker takes control of the signed-in app
    Given I am signed in
    Then a service worker controls the page

  Scenario: A navigation that cannot reach the server shows the offline page
    Given I am signed in
    And a service worker controls the page
    When the network goes offline
    And I visit "/entries"
    Then I see the offline page

  Scenario: The worker caches nothing that belongs to the reader
    Given I am signed in
    And a service worker controls the page
    Then the worker's cache holds nothing but public assets
