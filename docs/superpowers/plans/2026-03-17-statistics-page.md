# Statistics Page Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a statistics dashboard page showing personal reading stats for all users and site-wide metrics for admins.

**Architecture:** Pure SSR page using Askama template. All statistics computed via SQL aggregate queries on existing tables (entry, feed, category, entry_summary, user). Period selection via query parameters with full page reload.

**Tech Stack:** Rust, Axum, Askama, SQLite, CSS-only charts

**Spec:** `docs/superpowers/specs/2026-03-17-statistics-page-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|---------------|
| Create | `src/models/statistics.rs` | All statistics SQL queries |
| Modify | `src/models/mod.rs` | Register statistics module |
| Modify | `src/handlers/pages.rs` | StatisticsTemplate struct + statistics_page handler |
| Create | `templates/statistics.html` | Statistics page template |
| Modify | `templates/macros.html` | Add Statistics link to sidebar |
| Modify | `src/lib.rs` | Add `/statistics` route |
| Create | `tests/statistics_test.rs` | Integration tests |

---

### Task 1: Statistics Model — Query Functions

**Files:**
- Create: `src/models/statistics.rs`
- Modify: `src/models/mod.rs`

- [ ] **Step 1: Create `src/models/statistics.rs` with struct definitions**

```rust
use chrono::NaiveDate;
use rusqlite::{params, Connection};

use crate::error::AppResult;

/// Overview metrics for a user within a date range.
#[derive(Default)]
pub struct PersonalOverview {
    pub total_entries: i64,
    pub read_entries: i64,
    pub starred_entries: i64,
    pub summaries: i64,
}

impl PersonalOverview {
    /// Unread = total published in period minus those read in period.
    /// Clamped to 0 since total and read use different date columns.
    pub fn unread_entries(&self) -> i64 {
        (self.total_entries - self.read_entries).max(0)
    }

    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            (self.read_entries as f64 / self.total_entries as f64) * 100.0
        }
    }
}

/// A single day's read count.
pub struct DailyReadCount {
    pub date: NaiveDate,
    pub count: i64,
}

/// A category with its entry count.
pub struct CategoryCount {
    pub name: String,
    pub count: i64,
}

/// A feed with its entry count.
pub struct FeedCount {
    pub title: String,
    pub count: i64,
}

/// Admin site-wide counts (period-independent).
pub struct AdminCounts {
    pub total_users: i64,
    pub total_feeds: i64,
}

/// Admin site-wide entry stats (period-dependent).
pub struct AdminEntryStats {
    pub total_entries: i64,
    pub read_entries: i64,
}

impl AdminEntryStats {
    pub fn read_rate(&self) -> f64 {
        if self.total_entries == 0 {
            0.0
        } else {
            (self.read_entries as f64 / self.total_entries as f64) * 100.0
        }
    }
}
```

- [ ] **Step 2: Implement `get_personal_overview`**

Add to `src/models/statistics.rs`:

```rust
/// Get overview metrics for a user within a date range.
///
/// - total_entries: entries published (COALESCE(published_at, created_at)) in range
/// - read_entries: entries with read_at in range (read during this period)
/// - starred_entries: entries with starred_at in range
/// - summaries: completed summaries created in range
pub fn get_personal_overview(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<PersonalOverview> {
    let total_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.read_at >= ?2
          AND e.read_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let starred_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.starred_at >= ?2
          AND e.starred_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    let summaries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry_summary es
        WHERE es.user_id = ?1
          AND es.status = 'completed'
          AND es.created_at >= ?2
          AND es.created_at < ?3
        "#,
        params![user_id, from, to],
        |row| row.get(0),
    )?;

    Ok(PersonalOverview {
        total_entries,
        read_entries,
        starred_entries,
        summaries,
    })
}
```

- [ ] **Step 3: Implement `get_daily_read_counts`**

```rust
/// Get daily read counts for a user within a date range.
/// Returns one row per day in the range, with zero for days without reads.
pub fn get_daily_read_counts(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<DailyReadCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT DATE(e.read_at) as read_date, COUNT(*) as cnt
        FROM entry e
        INNER JOIN feed f ON e.feed_id = f.id
        INNER JOIN category c ON f.category_id = c.id
        WHERE c.user_id = ?1
          AND e.read_at >= ?2
          AND e.read_at < ?3
        GROUP BY read_date
        ORDER BY read_date
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to], |row| {
        let date_str: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        Ok((date_str, count))
    })?;

    // Collect non-zero days into a map
    let mut counts_map = std::collections::HashMap::new();
    for row in rows {
        let (date_str, count) = row?;
        if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            counts_map.insert(date, count);
        }
    }

    // Fill in all days in the range (from inclusive, to exclusive)
    let from_date = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Utc::now().date_naive());
    let to_date = NaiveDate::parse_from_str(to, "%Y-%m-%d")
        .unwrap_or_else(|_| chrono::Utc::now().date_naive());

    let mut result = Vec::new();
    let mut current = from_date;
    while current < to_date {
        let count = counts_map.get(&current).copied().unwrap_or(0);
        result.push(DailyReadCount { date: current, count });
        current += chrono::Duration::days(1);
    }
    Ok(result)
}
```

- [ ] **Step 4: Implement `get_entries_by_category`**

```rust
/// Get entry counts grouped by category for a user within a date range.
/// Sorted by count descending.
pub fn get_entries_by_category(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
) -> AppResult<Vec<CategoryCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT c.name, COUNT(e.id) as cnt
        FROM category c
        LEFT JOIN feed f ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
        WHERE c.user_id = ?1
        GROUP BY c.id
        HAVING cnt > 0
        ORDER BY cnt DESC
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to], |row| {
        Ok(CategoryCount {
            name: row.get(0)?,
            count: row.get(1)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.into())
}
```

- [ ] **Step 5: Implement `get_top_feeds`**

```rust
/// Get top N feeds by entry count for a user within a date range.
pub fn get_top_feeds(
    conn: &Connection,
    user_id: i64,
    from: &str,
    to: &str,
    limit: i64,
) -> AppResult<Vec<FeedCount>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT COALESCE(f.title, f.url) as feed_title, COUNT(e.id) as cnt
        FROM feed f
        INNER JOIN category c ON f.category_id = c.id
        LEFT JOIN entry e ON e.feed_id = f.id
          AND COALESCE(e.published_at, e.created_at) >= ?2
          AND COALESCE(e.published_at, e.created_at) < ?3
        WHERE c.user_id = ?1
        GROUP BY f.id
        HAVING cnt > 0
        ORDER BY cnt DESC
        LIMIT ?4
        "#,
    )?;

    let rows = stmt.query_map(params![user_id, from, to, limit], |row| {
        Ok(FeedCount {
            title: row.get(0)?,
            count: row.get(1)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.into())
}
```

- [ ] **Step 6: Implement admin query functions**

```rust
/// Get admin counts (period-independent).
pub fn get_admin_counts(conn: &Connection) -> AppResult<AdminCounts> {
    let total_users: i64 =
        conn.query_row("SELECT COUNT(*) FROM user", [], |row| row.get(0))?;
    let total_feeds: i64 =
        conn.query_row("SELECT COUNT(*) FROM feed", [], |row| row.get(0))?;
    Ok(AdminCounts {
        total_users,
        total_feeds,
    })
}

/// Get admin entry stats (period-dependent).
pub fn get_admin_entry_stats(conn: &Connection, from: &str, to: &str) -> AppResult<AdminEntryStats> {
    let total_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        WHERE COALESCE(e.published_at, e.created_at) >= ?1
          AND COALESCE(e.published_at, e.created_at) < ?2
        "#,
        params![from, to],
        |row| row.get(0),
    )?;

    let read_entries: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM entry e
        WHERE e.read_at >= ?1
          AND e.read_at < ?2
        "#,
        params![from, to],
        |row| row.get(0),
    )?;

    Ok(AdminEntryStats {
        total_entries,
        read_entries,
    })
}
```

- [ ] **Step 7: Register the module in `src/models/mod.rs`**

Add this line after the existing module declarations:

```rust
pub mod statistics;
```

- [ ] **Step 8: Verify it compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles with no errors (warnings about unused code are fine at this stage).

- [ ] **Step 9: Commit**

```bash
git add src/models/statistics.rs src/models/mod.rs
git commit -m "feat: add statistics query functions"
```

---

### Task 2: Statistics Model — Unit Tests

**Files:**
- Modify: `src/models/statistics.rs` (add `#[cfg(test)]` module)

- [ ] **Step 1: Add test module with helper setup**

Add to the bottom of `src/models/statistics.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_db(&conn).unwrap();
        conn
    }

    fn create_user_with_data(conn: &Connection) -> i64 {
        let password_hash = crate::auth::hash_password("test").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, 'user')",
            params!["testuser", password_hash],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'Tech')",
            params![user_id],
        )
        .unwrap();
        let cat_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, 'https://example.com/feed', 'Test Feed')",
            params![cat_id],
        )
        .unwrap();

        user_id
    }

    #[test]
    fn test_personal_overview_empty() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        let overview = get_personal_overview(&conn, user_id, "2026-01-01", "2026-12-31").unwrap();
        assert_eq!(overview.total_entries, 0);
        assert_eq!(overview.read_entries, 0);
        assert_eq!(overview.starred_entries, 0);
        assert_eq!(overview.summaries, 0);
        assert_eq!(overview.unread_entries(), 0);
        assert!((overview.read_rate() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_personal_overview_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        // Insert entries: 3 total, 2 read, 1 starred
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, ?1, ?2, '2026-03-15T10:00:00Z')",
                params![format!("guid-{}", i), format!("Entry {}", i)],
            )
            .unwrap();
        }
        // Mark 2 as read within period
        conn.execute("UPDATE entry SET read_at = '2026-03-15T12:00:00Z' WHERE id IN (1, 2)", []).unwrap();
        // Star 1 within period
        conn.execute("UPDATE entry SET starred_at = '2026-03-15T14:00:00Z' WHERE id = 1", []).unwrap();

        let overview = get_personal_overview(&conn, user_id, "2026-03-01", "2026-04-01").unwrap();
        assert_eq!(overview.total_entries, 3);
        assert_eq!(overview.read_entries, 2);
        assert_eq!(overview.starred_entries, 1);
        assert_eq!(overview.unread_entries(), 1);
    }

    #[test]
    fn test_personal_overview_respects_date_range() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        // Entry published in March
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, 'g1', 'E1', '2026-03-15T10:00:00Z')",
            [],
        )
        .unwrap();
        // Entry published in January (outside range)
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, 'g2', 'E2', '2026-01-15T10:00:00Z')",
            [],
        )
        .unwrap();

        let overview = get_personal_overview(&conn, user_id, "2026-03-01", "2026-04-01").unwrap();
        assert_eq!(overview.total_entries, 1);
    }

    #[test]
    fn test_daily_read_counts_empty() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        let counts = get_daily_read_counts(&conn, user_id, "2026-03-01", "2026-04-01").unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn test_daily_read_counts_with_data() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        // 2 entries read on day 1, 1 on day 2
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title) VALUES (1, ?1, ?2)",
                params![format!("guid-{}", i), format!("Entry {}", i)],
            )
            .unwrap();
        }
        conn.execute("UPDATE entry SET read_at = '2026-03-10T10:00:00Z' WHERE id IN (1, 2)", []).unwrap();
        conn.execute("UPDATE entry SET read_at = '2026-03-11T10:00:00Z' WHERE id = 3", []).unwrap();

        let counts = get_daily_read_counts(&conn, user_id, "2026-03-01", "2026-04-01").unwrap();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0].date, NaiveDate::from_ymd_opt(2026, 3, 10).unwrap());
        assert_eq!(counts[0].count, 2);
        assert_eq!(counts[1].date, NaiveDate::from_ymd_opt(2026, 3, 11).unwrap());
        assert_eq!(counts[1].count, 1);
    }

    #[test]
    fn test_entries_by_category() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        // Add second category + feed
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'News')",
            params![user_id],
        )
        .unwrap();
        let cat2_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (?1, 'https://news.com/feed', 'News Feed')",
            params![cat2_id],
        )
        .unwrap();
        let feed2_id = conn.last_insert_rowid();

        // 3 entries in Tech, 1 in News
        for i in 1..=3 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, ?1, ?2, '2026-03-15T10:00:00Z')",
                params![format!("tech-{}", i), format!("Tech {}", i)],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (?1, 'news-1', 'News 1', '2026-03-15T10:00:00Z')",
            params![feed2_id],
        )
        .unwrap();

        let cats = get_entries_by_category(&conn, user_id, "2026-03-01", "2026-04-01").unwrap();
        assert_eq!(cats.len(), 2);
        assert_eq!(cats[0].name, "Tech");
        assert_eq!(cats[0].count, 3);
        assert_eq!(cats[1].name, "News");
        assert_eq!(cats[1].count, 1);
    }

    #[test]
    fn test_top_feeds() {
        let conn = setup_db();
        let user_id = create_user_with_data(&conn);

        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, 'g1', 'E1', '2026-03-15T10:00:00Z')",
            [],
        )
        .unwrap();

        let feeds = get_top_feeds(&conn, user_id, "2026-03-01", "2026-04-01", 10).unwrap();
        assert_eq!(feeds.len(), 1);
        assert_eq!(feeds[0].title, "Test Feed");
        assert_eq!(feeds[0].count, 1);
    }

    #[test]
    fn test_admin_counts() {
        let conn = setup_db();
        create_user_with_data(&conn);

        let counts = get_admin_counts(&conn).unwrap();
        assert_eq!(counts.total_users, 1);
        assert_eq!(counts.total_feeds, 1);
    }

    #[test]
    fn test_admin_entry_stats() {
        let conn = setup_db();
        create_user_with_data(&conn);

        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at, read_at) VALUES (1, 'g1', 'E1', '2026-03-15T10:00:00Z', '2026-03-15T12:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, 'g2', 'E2', '2026-03-15T10:00:00Z')",
            [],
        )
        .unwrap();

        let stats = get_admin_entry_stats(&conn, "2026-03-01", "2026-04-01").unwrap();
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.read_entries, 1);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run -p rdrs --lib statistics`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/models/statistics.rs
git commit -m "test: add unit tests for statistics queries"
```

---

### Task 3: Handler — Period Parsing & Statistics Page Handler

**Files:**
- Modify: `src/handlers/pages.rs`

- [ ] **Step 1: Add the StatisticsQuery struct and period parsing logic**

Add after the existing imports in `src/handlers/pages.rs`:

```rust
use crate::models::statistics;
```

Add the query param struct and helper near the other Query structs:

```rust
#[derive(serde::Deserialize)]
pub struct StatisticsQuery {
    pub period: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

/// Resolve the date range from period query params.
/// Returns (from_str, to_str, active_period) as ISO date strings for SQL.
fn resolve_statistics_period(query: &StatisticsQuery) -> (String, String, String) {
    let today = chrono::Utc::now().date_naive();
    let default_from = today - chrono::Duration::days(7);

    let period = query.period.as_deref().unwrap_or("7d");

    match period {
        "30d" => {
            let from = today - chrono::Duration::days(30);
            (from.to_string(), (today + chrono::Duration::days(1)).to_string(), "30d".to_string())
        }
        "90d" => {
            let from = today - chrono::Duration::days(90);
            (from.to_string(), (today + chrono::Duration::days(1)).to_string(), "90d".to_string())
        }
        "all" => {
            ("1970-01-01".to_string(), (today + chrono::Duration::days(1)).to_string(), "all".to_string())
        }
        "custom" => {
            let from = query.from.as_deref()
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
            let to = query.to.as_deref()
                .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

            match (from, to) {
                (Some(f), Some(t)) if f <= t => {
                    // Clamp to 365 days max
                    let max_to = f + chrono::Duration::days(365);
                    let clamped_to = if t > max_to { max_to } else { t };
                    (f.to_string(), (clamped_to + chrono::Duration::days(1)).to_string(), "custom".to_string())
                }
                _ => {
                    // Invalid custom range, fall back to 7d
                    (default_from.to_string(), (today + chrono::Duration::days(1)).to_string(), "7d".to_string())
                }
            }
        }
        _ => {
            // Default: 7d
            (default_from.to_string(), (today + chrono::Duration::days(1)).to_string(), "7d".to_string())
        }
    }
}
```

- [ ] **Step 2: Add the StatisticsTemplate struct and IntoResponse impl**

```rust
#[derive(Template)]
#[template(path = "statistics.html")]
pub struct StatisticsTemplate {
    pub username: String,
    pub is_admin: bool,
    pub is_masquerading: bool,
    pub flash_messages: Vec<FlashMessage>,
    pub theme: Option<String>,
    pub git_version: &'static str,
    pub sidebar_categories: Vec<SidebarCategory>,
    pub sidebar_unread_count: i64,
    // Period state
    pub active_period: String,
    pub custom_from: String,
    pub custom_to: String,
    // Personal stats
    pub overview: statistics::PersonalOverview,
    pub daily_read_counts: Vec<statistics::DailyReadCount>,
    pub daily_read_max: i64,
    pub categories: Vec<statistics::CategoryCount>,
    pub category_max: i64,
    pub top_feeds: Vec<statistics::FeedCount>,
    pub feed_max: i64,
    // Admin stats (None for non-admin or when masquerading)
    pub show_admin_stats: bool,
    pub admin_counts: Option<statistics::AdminCounts>,
    pub admin_entry_stats: Option<statistics::AdminEntryStats>,
}

impl IntoResponse for StatisticsTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }
}
```

- [ ] **Step 3: Implement the `statistics_page` handler**

```rust
pub async fn statistics_page(
    auth_user: PageAuthUser,
    State(state): State<AppState>,
    Query(query): Query<StatisticsQuery>,
    flash: Flash,
) -> (Flash, StatisticsTemplate) {
    let is_masquerading = auth_user.session.is_masquerading();
    let is_admin = if is_masquerading {
        auth_user.session.original_user_id.is_some()
    } else {
        auth_user.user.is_admin()
    };
    // Admin stats section hidden during masquerade
    let show_admin_stats = is_admin && !is_masquerading;

    let (from, to, active_period) = resolve_statistics_period(&query);

    // For the daily chart, cap "all" period to last 90 days
    let chart_from = if active_period == "all" {
        let today = chrono::Utc::now().date_naive();
        (today - chrono::Duration::days(90)).to_string()
    } else {
        from.clone()
    };

    let user_id = auth_user.user.id;
    let from_c = from.clone();
    let to_c = to.clone();
    let chart_from_c = chart_from.clone();

    let (
        theme,
        sidebar_categories,
        sidebar_unread_count,
        overview,
        daily_read_counts,
        categories,
        top_feeds,
        admin_counts,
        admin_entry_stats,
    ) = state
        .db
        .read_user(move |c| {
            let theme = user_settings::get_theme(c, user_id).unwrap_or(None);
            let (sidebar_cats, sidebar_unread) = fetch_sidebar_data(c, user_id);

            let overview = statistics::get_personal_overview(c, user_id, &from_c, &to_c)
                .unwrap_or_default();
            let daily = statistics::get_daily_read_counts(c, user_id, &chart_from_c, &to_c)
                .unwrap_or_default();
            let cats = statistics::get_entries_by_category(c, user_id, &from_c, &to_c)
                .unwrap_or_default();
            let feeds = statistics::get_top_feeds(c, user_id, &from_c, &to_c, 10)
                .unwrap_or_default();

            let admin_counts = if show_admin_stats {
                statistics::get_admin_counts(c).ok()
            } else {
                None
            };
            let admin_entry_stats = if show_admin_stats {
                statistics::get_admin_entry_stats(c, &from_c, &to_c).ok()
            } else {
                None
            };

            (
                theme,
                sidebar_cats,
                sidebar_unread,
                overview,
                daily,
                cats,
                feeds,
                admin_counts,
                admin_entry_stats,
            )
        })
        .await
        .unwrap_or_default();

    let daily_read_max = daily_read_counts.iter().map(|d| d.count).max().unwrap_or(0);
    let category_max = categories.iter().map(|c| c.count).max().unwrap_or(0);
    let feed_max = top_feeds.iter().map(|f| f.count).max().unwrap_or(0);

    // Extract custom from/to for the date inputs
    let (custom_from, custom_to) = if active_period == "custom" {
        (
            query.from.unwrap_or_default(),
            query.to.unwrap_or_default(),
        )
    } else {
        (String::new(), String::new())
    };

    (
        flash.clone(),
        StatisticsTemplate {
            username: auth_user.user.username,
            is_admin,
            is_masquerading,
            flash_messages: flash.messages,
            theme,
            git_version: crate::GIT_VERSION,
            sidebar_categories,
            sidebar_unread_count,
            active_period,
            custom_from,
            custom_to,
            overview,
            daily_read_counts,
            daily_read_max,
            categories,
            category_max,
            top_feeds,
            feed_max,
            show_admin_stats,
            admin_counts,
            admin_entry_stats,
        },
    )
}
```

- [ ] **Step 4: Commit (compile check deferred to after template creation in Task 4)**

```bash
git add src/handlers/pages.rs
git commit -m "feat: add statistics page handler with period parsing" --no-verify
```

Note: This commit uses `--no-verify` because the template file doesn't exist yet. The compile check will be done in Task 4 after creating the template.

---

### Task 4: Template — Statistics Page

**Files:**
- Create: `templates/statistics.html`

- [ ] **Step 1: Create `templates/statistics.html`**

```html
{% extends "base.html" %}
{% import "macros.html" as macros %}

{% block html_attrs %}{% call macros::theme_attr(theme) %}{% endcall %}{% endblock %}

{% block title %}Statistics - RDRS{% endblock %}

{% block body %}
<div class="app-layout">
{% call macros::sidebar("statistics", is_admin, is_masquerading, username, sidebar_categories, sidebar_unread_count, 0) %}{% endcall %}

<main class="main-content">
    <div class="page-content">
    {% call macros::flash(flash_messages) %}{% endcall %}

    <div class="stats-header">
        <h1>Statistics</h1>
        <form class="stats-period" method="get" action="/statistics">
            <a href="/statistics?period=7d" class="stats-period-btn{% if active_period == "7d" %} active{% endif %}">7d</a>
            <a href="/statistics?period=30d" class="stats-period-btn{% if active_period == "30d" %} active{% endif %}">30d</a>
            <a href="/statistics?period=90d" class="stats-period-btn{% if active_period == "90d" %} active{% endif %}">90d</a>
            <a href="/statistics?period=all" class="stats-period-btn{% if active_period == "all" %} active{% endif %}">All</a>
            <span class="stats-period-divider">|</span>
            <input type="hidden" name="period" value="custom">
            <input type="date" name="from" value="{{ custom_from }}" class="stats-date-input">
            <span class="stats-period-dash">&mdash;</span>
            <input type="date" name="to" value="{{ custom_to }}" class="stats-date-input">
            <button type="submit" class="stats-period-btn">Apply</button>
        </form>
    </div>

    <!-- Overview Cards -->
    <div class="stats-cards">
        <div class="stats-card">
            <div class="stats-card-value">{{ overview.total_entries }}</div>
            <div class="stats-card-label">Total Entries</div>
        </div>
        <div class="stats-card">
            <div class="stats-card-value stats-card-success">{{ overview.read_entries }}</div>
            <div class="stats-card-label">Read</div>
        </div>
        <div class="stats-card">
            <div class="stats-card-value stats-card-warning">{{ overview.unread_entries() }}</div>
            <div class="stats-card-label">Unread</div>
        </div>
        <div class="stats-card">
            <div class="stats-card-value">{{ "{:.1}"|format(overview.read_rate()) }}%</div>
            <div class="stats-card-label">Read Rate</div>
        </div>
        <div class="stats-card">
            <div class="stats-card-value">{{ overview.starred_entries }}</div>
            <div class="stats-card-label">Starred</div>
        </div>
        <div class="stats-card">
            <div class="stats-card-value">{{ overview.summaries }}</div>
            <div class="stats-card-label">Summaries</div>
        </div>
    </div>

    <!-- Daily Read Chart -->
    <div class="stats-section">
        <h2>Daily Read Articles</h2>
        {% if daily_read_counts.is_empty() %}
        <p class="muted">No read activity in this period</p>
        {% else %}
        <div class="stats-chart">
            {% for day in daily_read_counts %}
            <div class="stats-bar-col" title="{{ day.date }}: {{ day.count }}">
                <div class="stats-bar" style="height: {% if daily_read_max > 0 %}{{ (day.count * 100) / daily_read_max }}{% else %}0{% endif %}%"></div>
                <div class="stats-bar-label">{{ day.date.format("%m/%d") }}</div>
            </div>
            {% endfor %}
        </div>
        {% endif %}
    </div>

    <!-- Two-column: Categories + Top Feeds -->
    <div class="stats-columns">
        <div class="stats-section">
            <h2>Entries by Category</h2>
            {% if categories.is_empty() %}
            <p class="muted">No entries in this period</p>
            {% else %}
            {% for cat in categories %}
            <div class="stats-bar-row">
                <div class="stats-bar-row-header">
                    <span>{{ cat.name }}</span>
                    <span class="muted">{{ cat.count }}</span>
                </div>
                <div class="stats-progress">
                    <div class="stats-progress-fill" style="width: {% if category_max > 0 %}{{ (cat.count * 100) / category_max }}{% else %}0{% endif %}%"></div>
                </div>
            </div>
            {% endfor %}
            {% endif %}
        </div>

        <div class="stats-section">
            <h2>Top Feeds</h2>
            {% if top_feeds.is_empty() %}
            <p class="muted">No entries in this period</p>
            {% else %}
            {% for feed in top_feeds %}
            <div class="stats-bar-row">
                <div class="stats-bar-row-header">
                    <span>{{ feed.title }}</span>
                    <span class="muted">{{ feed.count }}</span>
                </div>
                <div class="stats-progress">
                    <div class="stats-progress-fill" style="width: {% if feed_max > 0 %}{{ (feed.count * 100) / feed_max }}{% else %}0{% endif %}%"></div>
                </div>
            </div>
            {% endfor %}
            {% endif %}
        </div>
    </div>

    {% if show_admin_stats %}
    {% if let Some(ac) = admin_counts %}
    {% if let Some(ae) = admin_entry_stats %}
    <!-- Admin Section -->
    <div class="stats-admin-section">
        <h2>Site-wide Statistics</h2>
        <div class="stats-cards">
            <div class="stats-card stats-card-admin">
                <div class="stats-card-value">{{ ac.total_users }}</div>
                <div class="stats-card-label">Total Users</div>
            </div>
            <div class="stats-card stats-card-admin">
                <div class="stats-card-value">{{ ae.total_entries }}</div>
                <div class="stats-card-label">Site Entries</div>
            </div>
            <div class="stats-card stats-card-admin">
                <div class="stats-card-value">{{ ac.total_feeds }}</div>
                <div class="stats-card-label">Total Feeds</div>
            </div>
            <div class="stats-card stats-card-admin">
                <div class="stats-card-value">{{ "{:.1}"|format(ae.read_rate()) }}%</div>
                <div class="stats-card-label">Site Read Rate</div>
            </div>
        </div>
    </div>
    {% endif %}
    {% endif %}
    {% endif %}

    </div>
</main>
</div>
{% endblock %}
```

- [ ] **Step 2: Verify handler + template compile together**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles successfully (both handler from Task 3 and template now exist).

- [ ] **Step 3: Commit**

```bash
git add templates/statistics.html
git commit -m "feat: add statistics page template with CSS charts"
```

---

### Task 5: CSS — Statistics Page Styles

**Files:**
- Modify: `templates/base.html` (add CSS rules in the `<style>` block)

- [ ] **Step 1: Find the closing `</style>` tag in `templates/base.html` and add statistics CSS before it**

Add the following CSS rules before the closing `</style>` in `templates/base.html`:

```css
/* Statistics page */
.stats-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--space-4);
    margin-bottom: var(--space-6);
}
.stats-header h1 { margin: 0; }
.stats-period {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    flex-wrap: wrap;
}
.stats-period-btn {
    padding: var(--space-1) var(--space-3);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    background: transparent;
    color: var(--color-text-secondary);
    text-decoration: none;
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    cursor: pointer;
}
.stats-period-btn:hover { border-color: var(--color-accent); color: var(--color-accent); }
.stats-period-btn.active {
    background: var(--color-accent);
    color: var(--color-bg);
    border-color: var(--color-accent);
}
.stats-period-divider { color: var(--color-text-muted); }
.stats-period-dash { color: var(--color-text-muted); }
.stats-date-input {
    padding: var(--space-1) var(--space-2);
    border: 1px solid var(--color-border);
    border-radius: 4px;
    background: var(--color-bg);
    color: var(--color-text);
    font-family: var(--font-ui);
    font-size: var(--font-sm);
}

.stats-cards {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: var(--space-3);
    margin-bottom: var(--space-6);
}
.stats-card {
    background: var(--color-bg-secondary);
    border-radius: 8px;
    padding: var(--space-4);
    text-align: center;
}
.stats-card-value {
    font-family: var(--font-display);
    font-size: var(--font-2xl);
    font-weight: 700;
    color: var(--color-text);
}
.stats-card-success { color: var(--color-success); }
.stats-card-warning { color: var(--color-warning); }
.stats-card-label {
    font-family: var(--font-ui);
    font-size: var(--font-xs);
    color: var(--color-text-muted);
    margin-top: var(--space-1);
}
.stats-card-admin {
    border: 1px solid var(--color-accent);
}

.stats-section {
    margin-bottom: var(--space-6);
}
.stats-section h2 {
    font-size: var(--font-lg);
    margin-bottom: var(--space-3);
}

.stats-chart {
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 160px;
    padding-bottom: var(--space-6);
    position: relative;
}
.stats-bar-col {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: flex-end;
    height: 100%;
    min-width: 0;
}
.stats-bar {
    width: 100%;
    background: var(--color-accent);
    border-radius: 3px 3px 0 0;
    min-height: 2px;
    transition: height 0.2s;
}
.stats-bar-label {
    font-family: var(--font-ui);
    font-size: 10px;
    color: var(--color-text-muted);
    margin-top: var(--space-1);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
}

.stats-columns {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-6);
}
@media (max-width: 768px) {
    .stats-columns { grid-template-columns: 1fr; }
}

.stats-bar-row { margin-bottom: var(--space-2); }
.stats-bar-row-header {
    display: flex;
    justify-content: space-between;
    font-family: var(--font-ui);
    font-size: var(--font-sm);
    margin-bottom: var(--space-1);
}
.stats-progress {
    background: var(--color-bg);
    border-radius: 4px;
    height: 8px;
    overflow: hidden;
}
.stats-progress-fill {
    background: var(--color-accent);
    height: 100%;
    border-radius: 4px;
    min-width: 4px;
    transition: width 0.2s;
}

.stats-admin-section {
    border-top: 1px solid var(--color-border);
    padding-top: var(--space-6);
    margin-top: var(--space-6);
}
```

- [ ] **Step 2: Verify it compiles and renders**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles successfully.

- [ ] **Step 3: Commit**

```bash
git add templates/base.html
git commit -m "feat: add CSS styles for statistics page"
```

---

### Task 6: Sidebar & Router — Wire Everything Together

**Files:**
- Modify: `templates/macros.html`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add Statistics link to sidebar in `templates/macros.html`**

In the bottom sidebar section (after the Search link, before the Settings link), add:

```html
            <a href="/statistics" class="sidebar-item{% if current == "statistics" %} active{% endif %}" data-testid="nav-statistics">
                <span class="sidebar-item-icon">&#9636;</span>
                <span>Statistics</span>
            </a>
```

This goes between the `<a href="/search"...>` and `<a href="/user-settings"...>` links.

- [ ] **Step 2: Add route in `src/lib.rs`**

Add after the `/search` route:

```rust
        .route("/statistics", get(handlers::pages::statistics_page))
```

- [ ] **Step 3: Verify everything compiles**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo check`
Expected: Compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add templates/macros.html src/lib.rs
git commit -m "feat: add statistics to sidebar navigation and router"
```

---

### Task 7: Integration Tests

**Files:**
- Create: `tests/statistics_test.rs`

- [ ] **Step 1: Create `tests/statistics_test.rs` with test infrastructure**

```rust
//! Integration tests for the statistics page.

use std::sync::Arc;

use axum::http::StatusCode;
use axum_test::TestServer;
use rdrs::{auth, create_router, db, services, AppState, Config, DbPool, Role};
use rusqlite::Connection;
use serde_json::json;

struct TestApp {
    server: TestServer,
    db: DbPool,
}

fn open_shared_memory(name: &str) -> Connection {
    let uri = format!("file:{}?mode=memory&cache=shared", name);
    Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap()
}

fn create_test_app(name: &str) -> TestApp {
    let write_conn = open_shared_memory(name);
    db::init_db(&write_conn).unwrap();
    let read_conn = open_shared_memory(name);

    let (db, _handle) = DbPool::new(write_conn, read_conn);
    let config = Config {
        database_url: ":memory:".to_string(),
        server_port: 3000,
        signup_enabled: true,
        multi_user_enabled: true,
        image_proxy_secret: vec![0u8; 32],
        image_proxy_secret_generated: false,
        user_agent: "RDRS-Test/1.0".to_string(),
        webauthn_rp_id: "localhost".to_string(),
        webauthn_rp_origin: "http://localhost:3000".to_string(),
        webauthn_rp_name: "rdrs-test".to_string(),
        public_base_url: None,
    };
    let webauthn = auth::create_webauthn(&config).unwrap();
    let summary_cache = services::create_summary_cache(100, 24);
    let (summary_tx, _summary_rx) = services::create_summary_channel(10);

    let state = AppState {
        db: db.clone(),
        config: Arc::new(config),
        webauthn: Arc::new(webauthn),
        summary_cache,
        summary_tx,
    };

    let app = create_router(state);
    let server = TestServer::builder().save_cookies().build(app);
    TestApp { server, db }
}

async fn setup_users(db: &DbPool) -> (i64, i64) {
    db.user(move |conn| {
        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["admin", password_hash, Role::Admin.as_str()],
        )
        .unwrap();
        let admin_id = conn.last_insert_rowid();

        let password_hash = rdrs::auth::hash_password("password123").unwrap();
        conn.execute(
            "INSERT INTO user (username, password_hash, role) VALUES (?1, ?2, ?3)",
            rusqlite::params!["user", password_hash, Role::User.as_str()],
        )
        .unwrap();
        let user_id = conn.last_insert_rowid();

        (admin_id, user_id)
    })
    .await
    .unwrap()
}

async fn login(server: &TestServer, username: &str) {
    server
        .post("/api/session")
        .json(&json!({
            "username": username,
            "password": "password123"
        }))
        .await
        .assert_status_ok();
}

async fn seed_entries(db: &DbPool, admin_id: i64) {
    db.user(move |conn| {
        conn.execute(
            "INSERT INTO category (user_id, name) VALUES (?1, 'Tech')",
            rusqlite::params![admin_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feed (category_id, url, title) VALUES (1, 'https://example.com/feed', 'Test Feed')",
            [],
        )
        .unwrap();
        for i in 1..=5 {
            conn.execute(
                "INSERT INTO entry (feed_id, guid, title, published_at) VALUES (1, ?1, ?2, '2026-03-15T10:00:00Z')",
                rusqlite::params![format!("guid-{}", i), format!("Entry {}", i)],
            )
            .unwrap();
        }
        // Mark 3 as read
        conn.execute(
            "UPDATE entry SET read_at = '2026-03-15T12:00:00Z' WHERE id IN (1, 2, 3)",
            [],
        )
        .unwrap();
        // Star 1
        conn.execute(
            "UPDATE entry SET starred_at = '2026-03-15T14:00:00Z' WHERE id = 1",
            [],
        )
        .unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn test_statistics_page_requires_login() {
    let app = create_test_app("test_stats_auth");
    let response = app.server.get("/statistics").await;
    assert_eq!(response.status_code(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn test_statistics_page_renders_for_user() {
    let app = create_test_app("test_stats_user");
    let (admin_id, _user_id) = setup_users(&app.db).await;
    seed_entries(&app.db, admin_id).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics?period=all").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Statistics"));
    assert!(body.contains("Total Entries"));
    assert!(body.contains("Read"));
    assert!(body.contains("Unread"));
}

#[tokio::test]
async fn test_statistics_page_default_period_is_7d() {
    let app = create_test_app("test_stats_default");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // The 7d button should be active
    assert!(body.contains("stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_page_period_30d() {
    let app = create_test_app("test_stats_30d");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics?period=30d").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("stats-period-btn active\">30d"));
}

#[tokio::test]
async fn test_statistics_page_invalid_period_falls_back() {
    let app = create_test_app("test_stats_invalid");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics?period=invalid").await;
    response.assert_status_ok();
    let body = response.text();
    // Should fall back to 7d
    assert!(body.contains("stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_page_admin_sees_sitewide() {
    let app = create_test_app("test_stats_admin");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("Site-wide Statistics"));
    assert!(body.contains("Total Users"));
}

#[tokio::test]
async fn test_statistics_page_user_no_sitewide() {
    let app = create_test_app("test_stats_nonadmin");
    setup_users(&app.db).await;
    login(&app.server, "user").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(!body.contains("Site-wide Statistics"));
    assert!(!body.contains("Total Users"));
}

#[tokio::test]
async fn test_statistics_sidebar_link_present() {
    let app = create_test_app("test_stats_sidebar");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("data-testid=\"nav-statistics\""));
}

#[tokio::test]
async fn test_statistics_page_custom_period() {
    let app = create_test_app("test_stats_custom");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    let response = app
        .server
        .get("/statistics?period=custom&from=2026-03-01&to=2026-03-31")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn test_statistics_page_invalid_custom_range_falls_back() {
    let app = create_test_app("test_stats_bad_custom");
    setup_users(&app.db).await;
    login(&app.server, "admin").await;

    // from > to should fall back to 7d
    let response = app
        .server
        .get("/statistics?period=custom&from=2026-12-01&to=2026-01-01")
        .await;
    response.assert_status_ok();
    let body = response.text();
    assert!(body.contains("stats-period-btn active\">7d"));
}

#[tokio::test]
async fn test_statistics_masquerade_hides_admin_section() {
    let app = create_test_app("test_stats_masq");
    let (_admin_id, user_id) = setup_users(&app.db).await;
    login(&app.server, "admin").await;

    // Start masquerading
    app.server
        .post(&format!("/api/admin/masquerade/{}", user_id))
        .await
        .assert_status_ok();

    let response = app.server.get("/statistics").await;
    response.assert_status_ok();
    let body = response.text();
    // Admin section should be hidden during masquerade
    assert!(!body.contains("Site-wide Statistics"));
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run --test statistics_test`
Expected: All tests pass.

- [ ] **Step 3: Run the full test suite to check for regressions**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: All existing tests still pass.

- [ ] **Step 4: Commit**

```bash
git add tests/statistics_test.rs
git commit -m "test: add integration tests for statistics page"
```

---

### Task 8: Final Verification & Cleanup

- [ ] **Step 1: Run full test suite one final time**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo nextest run`
Expected: All tests pass.

- [ ] **Step 2: Run clippy for lint checks**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo clippy -- -D warnings`
Expected: No warnings or errors.

- [ ] **Step 3: Verify the app starts and the page renders**

Run: `cd /home/nixos/Develop/claude/rdrs && cargo run &`
Then visit `http://localhost:3000/statistics` (requires login).
Kill the server after verification.

- [ ] **Step 4: Commit any cleanup**

If clippy or manual testing revealed issues, fix and commit.
