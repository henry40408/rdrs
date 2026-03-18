# Feed Health Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enhance the Feeds page with feed health info (last fetched, last updated, freshness status) and migrate all filtering/sorting from client-side JS to server-side query parameters.

**Architecture:** Modify existing `/feeds` handler to parse query params for filter/sort, compute relative timestamps and freshness, then render via the existing Askama template. Remove client-side filter/sort JS. No new routes, no new DB tables.

**Tech Stack:** Rust, Axum, Askama, SQLite, CSS

**Spec:** `docs/superpowers/specs/2026-03-18-feed-health-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `src/handlers/pages.rs` | FeedRow + FeedsTemplate changes, FeedsQuery, helper functions, filter/sort logic |
| Modify | `templates/feeds.html` | Server-driven filter bar, health info in rows, remove client-side filter JS |
| Modify | `templates/base.html` | Add freshness CSS classes |
| Modify | `tests/pages_test.rs` | Integration tests for filter, sort, health display |

---

### Task 1: Handler — Helper Functions and FeedRow Changes

**Files:**
- Modify: `src/handlers/pages.rs`

- [ ] **Step 1: Add the `format_relative_time` helper function**

Add near the top of `src/handlers/pages.rs` (after the existing helper functions like `escape_json_for_script`):

```rust
/// Format a datetime as a human-readable relative time string.
/// Returns (relative_text, iso_datetime_for_tooltip).
fn format_relative_time(dt: Option<chrono::DateTime<chrono::Utc>>) -> (String, String) {
    match dt {
        None => ("Never".to_string(), String::new()),
        Some(dt) => {
            let now = chrono::Utc::now();
            let duration = now.signed_duration_since(dt);
            let seconds = duration.num_seconds();
            let relative = if seconds < 60 {
                "Just now".to_string()
            } else if seconds < 3600 {
                let mins = duration.num_minutes();
                format!("{} minute{} ago", mins, if mins == 1 { "" } else { "s" })
            } else if seconds < 86400 {
                let hours = duration.num_hours();
                format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" })
            } else if seconds < 2_592_000 {
                // < 30 days
                let days = duration.num_days();
                format!("{} day{} ago", days, if days == 1 { "" } else { "s" })
            } else if seconds < 31_536_000 {
                // < 365 days
                let months = duration.num_days() / 30;
                format!("{} month{} ago", months, if months == 1 { "" } else { "s" })
            } else {
                let years = duration.num_days() / 365;
                format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
            };
            (relative, dt.to_rfc3339())
        }
    }
}

/// Compute freshness CSS class and key from feed_updated_at and fetched_at.
/// Returns (css_class, freshness_key).
/// freshness_key is "fresh", "warning", or "stale" — used for filtering.
fn compute_freshness(
    feed_updated_at: Option<chrono::DateTime<chrono::Utc>>,
    fetched_at: Option<chrono::DateTime<chrono::Utc>>,
) -> (String, String) {
    let now = chrono::Utc::now();
    match feed_updated_at {
        Some(updated) => {
            let days = (now - updated).num_days();
            if days <= 30 {
                (String::new(), "fresh".to_string())
            } else if days <= 90 {
                ("feed-freshness-warning".to_string(), "warning".to_string())
            } else {
                ("feed-freshness-stale".to_string(), "stale".to_string())
            }
        }
        None => {
            // No feed_updated_at — check fetched_at to determine health
            match fetched_at {
                Some(fetched) if (now - fetched).num_days() <= 30 => {
                    // Recently fetched but no date info — not a health problem
                    ("muted".to_string(), "fresh".to_string())
                }
                Some(fetched) if (now - fetched).num_days() <= 90 => {
                    // Fetched 31-90 days ago with no date info — warning
                    ("feed-freshness-warning".to_string(), "warning".to_string())
                }
                _ => {
                    // Never fetched or fetched >90 days ago — stale
                    ("feed-freshness-stale".to_string(), "stale".to_string())
                }
            }
        }
    }
}
```

- [ ] **Step 2: Add new fields to `FeedRow` struct**

Modify the existing `FeedRow` struct (line ~863) to add health fields. The struct derives `serde::Serialize` for `feed_data_json`, so add `#[serde(skip)]` to the new display-only fields:

```rust
#[derive(serde::Serialize)]
pub struct FeedRow {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub category_id: i64,
    pub category_name: String,
    pub has_icon: bool,
    pub fetch_error: Option<String>,
    pub description: Option<String>,
    pub site_url: Option<String>,
    pub custom_user_agent: Option<String>,
    pub http2_disabled: bool,
    pub custom_referrer: Option<String>,
    pub unread_count: i64,
    // Health fields — display only, excluded from feed_data_json
    #[serde(skip)]
    pub fetched_at_relative: String,
    #[serde(skip)]
    pub fetched_at_datetime: String,
    #[serde(skip)]
    pub feed_updated_at_relative: String,
    #[serde(skip)]
    pub feed_updated_at_datetime: String,
    #[serde(skip)]
    pub freshness_class: String,
    #[serde(skip)]
    pub freshness_key: String,
}
```

- [ ] **Step 3: Update FeedRow construction in `feeds_page` handler**

In the `feeds_page` handler (line ~952), update the `FeedRow` construction inside the closure to populate the new fields. The `Feed` struct from the model has `fetched_at: Option<DateTime<Utc>>` and `feed_updated_at: Option<DateTime<Utc>>`:

```rust
                .map(|f| {
                    let has_icon: i64 = c
                        .query_row(
                            "SELECT COUNT(*) FROM image WHERE entity_type = 'feed' AND entity_id = ?1",
                            [f.id],
                            |row| row.get(0),
                        )
                        .unwrap_or(0);
                    let (fetched_rel, fetched_dt) = format_relative_time(f.fetched_at);
                    let (updated_rel, updated_dt) = if f.feed_updated_at.is_some() {
                        format_relative_time(f.feed_updated_at)
                    } else if f.fetched_at.map(|ft| (chrono::Utc::now() - ft).num_days() <= 30).unwrap_or(false) {
                        ("No date info".to_string(), String::new())
                    } else {
                        ("Never".to_string(), String::new())
                    };
                    let (freshness_class, freshness_key) =
                        compute_freshness(f.feed_updated_at, f.fetched_at);
                    FeedRow {
                        title: f.title.clone().unwrap_or_else(|| f.url.clone()),
                        category_name: cat_map
                            .get(&f.category_id)
                            .cloned()
                            .unwrap_or_else(|| "Unknown".to_string()),
                        has_icon: has_icon > 0,
                        unread_count: *unread_map.get(&f.id).unwrap_or(&0),
                        id: f.id,
                        url: f.url,
                        category_id: f.category_id,
                        fetch_error: f.fetch_error,
                        description: f.description,
                        site_url: f.site_url,
                        custom_user_agent: f.custom_user_agent,
                        http2_disabled: f.http2_disabled,
                        custom_referrer: f.custom_referrer,
                        fetched_at_relative: fetched_rel,
                        fetched_at_datetime: fetched_dt,
                        feed_updated_at_relative: updated_rel,
                        feed_updated_at_datetime: updated_dt,
                        freshness_class,
                        freshness_key,
                    }
                })
```

- [ ] **Step 4: Verify it compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles (template hasn't changed yet so unused fields may warn, that's fine).

- [ ] **Step 5: Commit**

```bash
git add src/handlers/pages.rs
git commit -m "feat: add feed health helper functions and FeedRow fields"
```

---

### Task 2: Handler — Server-Side Filter and Sort

**Files:**
- Modify: `src/handlers/pages.rs`

- [ ] **Step 1: Add `FeedsQuery` struct and update `FeedsTemplate`**

Add the query struct near other Query structs:

```rust
#[derive(serde::Deserialize)]
pub struct FeedsQuery {
    pub category: Option<i64>,
    pub filter: Option<String>,
    pub sort: Option<String>,
}
```

Add new fields to `FeedsTemplate` (line ~886):

```rust
#[derive(Template)]
#[template(path = "feeds.html")]
pub struct FeedsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub feeds: Vec<FeedRow>,
    pub categories: Vec<CategoryOption>,
    pub feed_data_json: String,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
    // Server-side filter/sort state
    pub active_filter: String,
    pub active_sort: String,
    pub active_category: Option<i64>,
}
```

- [ ] **Step 2: Update `feeds_page` handler signature and add filter/sort logic**

Change the handler to accept `Query(query): Query<FeedsQuery>`:

```rust
pub async fn feeds_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<FeedsQuery>,
    flash: Flash,
) -> (Flash, FeedsTemplate) {
```

After building `feed_rows` and `cat_options` inside the `read_user` closure, add filter and sort logic outside the closure (after the `.await`):

```rust
    // Apply server-side filter
    let active_filter = query.filter.as_deref().unwrap_or("all").to_string();
    let active_sort = query.sort.as_deref().unwrap_or("title").to_string();
    let active_category = query.category;

    let mut feeds_data = feeds_data;

    // Filter by category
    if let Some(cat_id) = active_category {
        feeds_data.retain(|f| f.category_id == cat_id);
    }

    // Filter by health status
    match active_filter.as_str() {
        "errors" => feeds_data.retain(|f| f.fetch_error.is_some()),
        "stale" => feeds_data.retain(|f| f.freshness_key == "stale"),
        _ => {} // "all" — no filtering
    }

    // Sort
    match active_sort.as_str() {
        "unread" => feeds_data.sort_by(|a, b| b.unread_count.cmp(&a.unread_count)),
        "category" => feeds_data.sort_by(|a, b| a.category_name.cmp(&b.category_name)),
        _ => feeds_data.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
```

And add the new fields to the `FeedsTemplate` construction:

```rust
    (
        flash.clone(),
        FeedsTemplate {
            // ... existing fields ...
            active_filter,
            active_sort,
            active_category,
        },
    )
```

- [ ] **Step 3: Verify it compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add src/handlers/pages.rs
git commit -m "feat: add server-side filter and sort for feeds page"
```

---

### Task 3: Template — Server-Driven Filter Bar and Health Info

**Files:**
- Modify: `templates/feeds.html`

- [ ] **Step 1: Replace the client-side filter bar with server-driven controls**

Replace lines 51-75 (the existing `<div class="filter-bar">` block) with:

```html
    <div class="filter-bar">
        <div class="form-group form-group-inline">
            <label for="filter-category">Category</label>
            <select id="filter-category" onchange="window.location.href=this.value" class="select-auto">
                <option value="/feeds?filter={{ active_filter }}&sort={{ active_sort }}">All Categories ({{ feeds.len() }})</option>
                {% for cat in categories %}
                <option value="/feeds?category={{ cat.id }}&filter={{ active_filter }}&sort={{ active_sort }}"{% if active_category == Some(cat.id) %} selected{% endif %}>{{ cat.name }} ({{ cat.feed_count }})</option>
                {% endfor %}
            </select>
        </div>
        <div class="form-group form-group-inline">
            <label for="sort-by">Sort</label>
            <select id="sort-by" onchange="window.location.href=this.value" class="select-auto">
                <option value="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}filter={{ active_filter }}&sort=title"{% if active_sort == "title" %} selected{% endif %}>Title</option>
                <option value="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}filter={{ active_filter }}&sort=unread"{% if active_sort == "unread" %} selected{% endif %}>Unread Count</option>
                <option value="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}filter={{ active_filter }}&sort=category"{% if active_sort == "category" %} selected{% endif %}>Category</option>
            </select>
        </div>
        <div class="form-group form-group-inline feed-filter-links">
            <a href="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}sort={{ active_sort }}&filter=all" class="feed-filter-link{% if active_filter == "all" %} active{% endif %}">All</a>
            <a href="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}sort={{ active_sort }}&filter=errors" class="feed-filter-link{% if active_filter == "errors" %} active{% endif %}">Errors</a>
            <a href="/feeds?{% if let Some(cat) = active_category %}category={{ cat }}&{% endif %}sort={{ active_sort }}&filter=stale" class="feed-filter-link{% if active_filter == "stale" %} active{% endif %}">Stale</a>
        </div>
    </div>
```

- [ ] **Step 2: Add health info to feed rows**

Replace line 93-94 (the `<tr>` and Title `<td>`) with:

```html
            <tr id="row-feed-{{ feed.id }}" data-feed-id="{{ feed.id }}" {% if feed.fetch_error.is_some() %}class="feed-error-no-border"{% endif %}>
                <td data-label="Title"{% if feed.fetch_error.is_some() %} class="feed-error-no-border"{% endif %}>
                    {% if feed.has_icon %}<img src="/api/feeds/{{ feed.id }}/icon" alt="" class="feed-icon" onerror="this.style.display='none'">{% endif %}<span title="{{ feed.url }}">{{ feed.title }}</span>
                    <div class="feed-health-info">
                        <span class="muted" title="{{ feed.fetched_at_datetime }}">Fetched: {{ feed.fetched_at_relative }}</span>
                        &middot;
                        <span class="{{ feed.freshness_class }}" title="{{ feed.feed_updated_at_datetime }}">Updated: {{ feed.feed_updated_at_relative }}</span>
                    </div>
                </td>
```

Also remove the `data-category-id` and `data-has-error` attributes from the `<tr>` since they were only used for client-side filtering.

- [ ] **Step 3: Remove client-side filter/sort JavaScript**

Remove the following functions from the `<script>` block:
- `handleFilterChange()` (lines 227-250)
- `updateURL()` (lines 252-262)
- `restoreFilters()` IIFE (lines 264-274)

- [ ] **Step 4: Verify it compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add templates/feeds.html
git commit -m "feat: server-side filter/sort and health info in feeds template"
```

---

### Task 4: CSS — Freshness Classes

**Files:**
- Modify: `templates/base.html`

- [ ] **Step 1: Add freshness and filter CSS before the closing `</style>` in `templates/base.html`**

```css
/* Feed health */
.feed-health-info {
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    color: var(--color-text-muted);
    margin-top: var(--space-1);
}
.feed-freshness-warning { color: var(--color-warning); }
.feed-freshness-stale { color: var(--color-error); }

.feed-filter-links {
    display: flex;
    gap: var(--space-2);
    align-items: center;
}
.feed-filter-link {
    padding: var(--space-1) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    text-decoration: none;
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    color: var(--color-text-secondary);
}
.feed-filter-link:hover { border-color: var(--color-accent); color: var(--color-accent); }
.feed-filter-link.active {
    background: var(--color-accent);
    color: var(--color-bg);
    border-color: var(--color-accent);
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add templates/base.html
git commit -m "feat: add CSS for feed health and filter links"
```

---

### Task 5: Integration Tests

**Files:**
- Modify: `tests/pages_test.rs`

- [ ] **Step 1: Add integration tests for feed health and server-side filtering**

Add these tests to `tests/pages_test.rs`:

```rust
// ============================================================================
// Feed Health Tests
// ============================================================================

#[tokio::test]
async fn test_feeds_page_shows_health_info() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Health Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title, fetched_at, feed_updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    1,
                    "https://example.com/health.xml",
                    "Health Feed",
                    "2026-03-18T10:00:00Z",
                    "2026-03-17T10:00:00Z"
                ],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("feed-health-info"));
    assert!(body.contains("Fetched:"));
    assert!(body.contains("Updated:"));
}

#[tokio::test]
async fn test_feeds_page_filter_errors() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Filter Test"],
            )
            .unwrap();
            // Feed with error
            conn.execute(
                "INSERT INTO feed (category_id, url, title, fetch_error) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![1, "https://bad.com/feed.xml", "Bad Feed", "Connection refused"],
            )
            .unwrap();
            // Feed without error
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://good.com/feed.xml", "Good Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Filter=errors should only show the bad feed
    let response = app.server.get("/feeds?filter=errors").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Bad Feed"));
    assert!(!body.contains("Good Feed"));
}

#[tokio::test]
async fn test_feeds_page_filter_stale() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Stale Test"],
            )
            .unwrap();
            // Stale feed (updated 100 days ago)
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-100 days'))",
                rusqlite::params![1, "https://stale.com/feed.xml", "Stale Feed"],
            )
            .unwrap();
            // Fresh feed (updated today)
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![1, "https://fresh.com/feed.xml", "Fresh Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Filter=stale should only show the stale feed
    let response = app.server.get("/feeds?filter=stale").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Stale Feed"));
    assert!(!body.contains("Fresh Feed"));
}

#[tokio::test]
async fn test_feeds_page_sort_unread() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Sort Test"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://a.com/feed.xml", "AAA Feed"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://b.com/feed.xml", "BBB Feed"],
            )
            .unwrap();
            // Add unread entries to BBB Feed (id=2)
            for i in 1..=3 {
                conn.execute(
                    "INSERT INTO entry (feed_id, guid, title) VALUES (?1, ?2, ?3)",
                    rusqlite::params![2, format!("guid-{}", i), format!("Entry {}", i)],
                )
                .unwrap();
            }
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds?sort=unread").await;
    response.assert_status_ok();
    let body = response.text();
    // BBB Feed should appear before AAA Feed when sorted by unread (descending)
    let bbb_pos = body.find("BBB Feed").unwrap();
    let aaa_pos = body.find("AAA Feed").unwrap();
    assert!(bbb_pos < aaa_pos, "BBB Feed (3 unread) should come before AAA Feed (0 unread)");
}

#[tokio::test]
async fn test_feeds_page_freshness_classes() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Freshness Test"],
            )
            .unwrap();
            // Warning feed (updated 50 days ago)
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-50 days'))",
                rusqlite::params![1, "https://warn.com/feed.xml", "Warning Feed"],
            )
            .unwrap();
            // Stale feed (updated 100 days ago)
            conn.execute(
                "INSERT INTO feed (category_id, url, title, feed_updated_at) VALUES (?1, ?2, ?3, datetime('now', '-100 days'))",
                rusqlite::params![1, "https://stale.com/feed.xml", "Old Feed"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    let response = app.server.get("/feeds").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("feed-freshness-warning"));
    assert!(body.contains("feed-freshness-stale"));
}

#[tokio::test]
async fn test_feeds_page_filter_by_category() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;

    app.db
        .user(move |conn| {
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cat A"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO category (user_id, name) VALUES (?1, ?2)",
                rusqlite::params![1, "Cat B"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![1, "https://a.com/feed.xml", "Feed In A"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feed (category_id, url, title) VALUES (?1, ?2, ?3)",
                rusqlite::params![2, "https://b.com/feed.xml", "Feed In B"],
            )
            .unwrap();
        })
        .await
        .unwrap();

    login(&app.server, "admin").await;

    // Filter by category 1
    let response = app.server.get("/feeds?category=1").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Feed In A"));
    assert!(!body.contains("Feed In B"));
}

#[tokio::test]
async fn test_feeds_page_invalid_filter_defaults_to_all() {
    let app = create_test_app(default_test_config());
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/feeds?filter=invalid").await;
    response.assert_status_ok();
    // Should render without error, showing all feeds
}
```

- [ ] **Step 2: Run the new tests**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run --test pages_test`
Expected: All tests pass.

- [ ] **Step 3: Run the full test suite**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: All tests pass (no regressions).

- [ ] **Step 4: Commit**

```bash
git add tests/pages_test.rs
git commit -m "test: add integration tests for feed health and server-side filtering"
```

---

### Task 6: Final Verification & Cleanup

- [ ] **Step 1: Run full test suite**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo clippy -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Check formatting**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo fmt -- --check`
Expected: No formatting issues.

- [ ] **Step 4: Fix any issues and commit**

If clippy/fmt found issues, fix and commit.
