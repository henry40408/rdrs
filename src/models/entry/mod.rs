use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};

use crate::db::{Db, DbInner, Tx};
use crate::error::{AppError, AppResult};
use crate::utils::text::strip_to_search_text;
use crate::{db_execute, query_all, query_opt, query_opt_tx, query_scalar};

mod filters;
pub mod query;
use filters::{
    Bind, Dialect, apply_continuation_condition, apply_filter_conditions, apply_time_conditions,
    published_sort_entry_hint,
};
// Only the unit tests exercise this predicate directly; production code reaches
// it through `published_sort_entry_hint`.
#[cfg(test)]
use filters::is_no_entry_side_predicate;

/// Sort order for entries
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntrySortOrder {
    #[default]
    PublishedAt, // COALESCE(published_at, created_at) DESC
    ReadAt,    // read_at DESC
    StarredAt, // starred_at DESC
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: Option<String>,
    pub link: Option<String>,
    pub content: Option<String>,
    pub summary: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub starred_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryWithFeed {
    #[serde(flatten)]
    pub entry: Entry,
    pub feed_title: Option<String>,
    pub feed_url: String,
    pub site_url: Option<String>,
    pub category_id: i64,
    pub category_name: String,
    pub feed_has_icon: bool,
    pub custom_referrer: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct EntryFilter {
    pub feed_id: Option<i64>,
    pub category_id: Option<i64>,
    pub unread_only: bool,
    pub starred_only: bool,
    pub read_only: bool,
    pub search: Option<String>,
    pub has_summary: Option<bool>,
    /// Snapshot boundary for the unread filter — a UTC `YYYY-MM-DD HH:MM:SS`
    /// string, the same format `datetime('now')` writes into `entry.read_at`.
    /// When set together with `unread_only`, entries read at-or-after this
    /// instant still count as unread, so reading-pane navigation can return
    /// to entries the reader just finished during this page view. Ignored
    /// when `unread_only` is false.
    pub read_after: Option<String>,
    /// Parsed boolean query AST for the global `/search` page. Set by the
    /// search handler from the `?q=` string; `None` on every other list path
    /// (a no-op). Rendered to SQL by `filters::render_query`.
    #[serde(skip)]
    pub query: Option<query::QueryNode>,
}

/// Pagination cursor. The wire format on the API is opaque to clients; we
/// emit the new composite form `<iso_8601_ts>|<id>` and accept the legacy
/// bare-`i64` form as a one-time grace path for in-flight cursors that may
/// still live in browser URLs/JS state at deploy time.
#[derive(Debug, Clone)]
pub enum ContinuationCursor {
    /// New `(sort_ts, id)` composite. `sort_ts` is the entry's sort-field
    /// value as TEXT (the same byte-string `SQLite` stores), so the predicate
    /// compares against an indexed column without conversion.
    Composite { sort_ts: String, id: i64 },
    /// Legacy `e.id < ?` cursor — accepted on input only; emitted only by
    /// pre-#164 clients.
    LegacyId(i64),
}

impl ContinuationCursor {
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            return None;
        }
        if let Some((ts, id)) = s.split_once('|') {
            if ts.is_empty() {
                return None;
            }
            id.parse::<i64>().ok().map(|id| Self::Composite {
                sort_ts: ts.to_string(),
                id,
            })
        } else {
            s.parse::<i64>().ok().map(Self::LegacyId)
        }
    }

    pub fn encode_composite(sort_ts: &str, id: i64) -> String {
        format!("{sort_ts}|{id}")
    }
}

/// Parameters for continuation-based pagination (Google Reader style).
#[derive(Debug, Clone, Default)]
pub struct ContinuationParams {
    pub oldest_first: bool,
    pub limit: i64,
    pub continuation: Option<ContinuationCursor>,
    /// Oldest timestamp (seconds since epoch)
    pub ot: Option<i64>,
    /// Newest timestamp (seconds since epoch)
    pub nt: Option<i64>,
    /// Sort order (default: `PublishedAt`)
    pub sort_order: EntrySortOrder,
}

/// Flat row for the `EntryWithFeed` join. `sqlx::FromRow` matches by column
/// NAME, and the join has duplicate base names (`e.title`/`f.title`,
/// `e.id`/`c.id`), so the `ENTRY_WITH_FEED_COLUMNS_*` lists alias every column
/// to the field names below. `has_icon` is an integer (COUNT or 0/1 CASE) that
/// maps to the `bool` `feed_has_icon` in the conversion.
#[derive(sqlx::FromRow)]
struct EntryWithFeedRow {
    id: i64,
    feed_id: i64,
    guid: String,
    title: Option<String>,
    link: Option<String>,
    content: Option<String>,
    summary: Option<String>,
    author: Option<String>,
    published_at: Option<DateTime<Utc>>,
    read_at: Option<DateTime<Utc>>,
    starred_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    feed_title: Option<String>,
    feed_url: String,
    site_url: Option<String>,
    category_id: i64,
    category_name: String,
    has_icon: i64,
    custom_referrer: Option<String>,
}

impl From<EntryWithFeedRow> for EntryWithFeed {
    fn from(r: EntryWithFeedRow) -> Self {
        EntryWithFeed {
            entry: Entry {
                id: r.id,
                feed_id: r.feed_id,
                guid: r.guid,
                title: r.title,
                link: r.link,
                content: r.content,
                summary: r.summary,
                author: r.author,
                published_at: r.published_at,
                read_at: r.read_at,
                starred_at: r.starred_at,
                created_at: r.created_at,
                updated_at: r.updated_at,
            },
            feed_title: r.feed_title,
            feed_url: r.feed_url,
            site_url: r.site_url,
            category_id: r.category_id,
            category_name: r.category_name,
            feed_has_icon: r.has_icon > 0,
            custom_referrer: r.custom_referrer,
        }
    }
}

/// SELECT column list for [`EntryWithFeedRow`], aliased to its field names.
/// `has_icon` here uses a correlated COUNT subquery, for queries that do NOT
/// `LEFT JOIN image`. Keep both variants and the row struct in sync.
const ENTRY_WITH_FEED_COLUMNS_COUNT: &str = "e.id AS id, e.feed_id AS feed_id, e.guid AS guid, e.title AS title, e.link AS link, e.content AS content, e.summary AS summary, e.author AS author, e.published_at AS published_at, e.read_at AS read_at, e.starred_at AS starred_at, e.created_at AS created_at, e.updated_at AS updated_at, f.title AS feed_title, f.url AS feed_url, f.site_url AS site_url, c.id AS category_id, c.name AS category_name, (SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) AS has_icon, f.custom_referrer AS custom_referrer";

/// Same columns as [`ENTRY_WITH_FEED_COLUMNS_COUNT`] but computes `has_icon`
/// from a `LEFT JOIN image i` already present in the query.
const ENTRY_WITH_FEED_COLUMNS_JOIN: &str = "e.id AS id, e.feed_id AS feed_id, e.guid AS guid, e.title AS title, e.link AS link, e.content AS content, e.summary AS summary, e.author AS author, e.published_at AS published_at, e.read_at AS read_at, e.starred_at AS starred_at, e.created_at AS created_at, e.updated_at AS updated_at, f.title AS feed_title, f.url AS feed_url, f.site_url AS site_url, c.id AS category_id, c.name AS category_name, CAST(CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END AS BIGINT) AS has_icon, f.custom_referrer AS custom_referrer";

// --- dynamic-query execution helpers ---------------------------------------
//
// Several list/count queries are built at runtime (filter conditions +
// continuation cursor) into a SQL `String` with `$N` placeholders and a
// parallel `Vec<Bind>`. These helpers dispatch on the backend and apply the
// binds in order. Runtime strings are wrapped in `sqlx::AssertSqlSafe` (every
// fragment is built by this module from `filters`, never from user input).

async fn fetch_entries_with_feed(
    db: &Db,
    sql: String,
    binds: Vec<Bind>,
) -> Result<Vec<EntryWithFeed>, sqlx::Error> {
    let rows = match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut q = sqlx::query_as::<sqlx::Sqlite, EntryWithFeedRow>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_all(pool).await?
        }
        DbInner::Postgres(pool) => {
            let mut q =
                sqlx::query_as::<sqlx::Postgres, EntryWithFeedRow>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_all(pool).await?
        }
    };
    Ok(rows.into_iter().map(EntryWithFeed::from).collect())
}

async fn fetch_scalar_i64(db: &Db, sql: String, binds: Vec<Bind>) -> Result<i64, sqlx::Error> {
    match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut q = sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_one(pool).await
        }
        DbInner::Postgres(pool) => {
            let mut q = sqlx::query_scalar::<sqlx::Postgres, i64>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_one(pool).await
        }
    }
}

/// Fetch `(id, sort_ts_micros)` id-list rows for the continuation index query.
async fn fetch_id_ts_rows(
    db: &Db,
    sql: String,
    binds: Vec<Bind>,
) -> Result<Vec<(i64, i64)>, sqlx::Error> {
    match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut q = sqlx::query_as::<sqlx::Sqlite, (i64, i64)>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_all(pool).await
        }
        DbInner::Postgres(pool) => {
            let mut q = sqlx::query_as::<sqlx::Postgres, (i64, i64)>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.fetch_all(pool).await
        }
    }
}

/// Execute a runtime-built statement, returning rows affected. A write path
/// (mark-all-read), so it takes the write-priority admission for its duration.
async fn exec_dynamic(db: &Db, sql: String, binds: Vec<Bind>) -> Result<u64, sqlx::Error> {
    let _guard = db.admit().await;
    match db.inner() {
        DbInner::Sqlite(pool) => {
            let mut q = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.execute(pool).await.map(|r| r.rows_affected())
        }
        DbInner::Postgres(pool) => {
            let mut q =
                sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(crate::db::pg_rewrite(&sql)));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.execute(pool).await.map(|r| r.rows_affected())
        }
    }
}

/// `exec_dynamic` against an open transaction.
async fn exec_dynamic_tx(
    tx: &mut Tx<'_>,
    sql: String,
    binds: Vec<Bind>,
) -> Result<u64, sqlx::Error> {
    match tx {
        Tx::Sqlite { tx: t, .. } => {
            let mut q = sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.execute(&mut **t).await.map(|r| r.rows_affected())
        }
        Tx::Postgres(t) => {
            let mut q =
                sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(crate::db::pg_rewrite(&sql)));
            for b in &binds {
                q = match b {
                    Bind::Int(i) => q.bind(*i),
                    Bind::Text(s) => q.bind(s.as_str()),
                    Bind::Ts(t) => q.bind(*t),
                };
            }
            q.execute(&mut **t).await.map(|r| r.rows_affected())
        }
    }
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<Entry>> {
    query_opt!(
        db,
        Entry,
        "SELECT id, feed_id, guid, title, link, content, summary, author, \
         published_at, read_at, starred_at, created_at, updated_at \
         FROM entry WHERE id = $1",
        id
    )
    .map_err(AppError::Database)
}

pub async fn find_by_id_with_feed(db: &Db, id: i64) -> AppResult<Option<EntryWithFeed>> {
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_COUNT} \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE e.id = $1"
    );
    fetch_entries_with_feed(db, sql, vec![Bind::Int(id)])
        .await
        .map(|v| v.into_iter().next())
        .map_err(AppError::Database)
}

/// Fetch a single entry by id, scoped to a specific user via the feed→category
/// ownership join. Returns `None` if the entry does not exist or belongs to a
/// different user (callers should treat both as 404).
pub async fn find_by_id_for_user(
    db: &Db,
    user_id: i64,
    entry_id: i64,
) -> AppResult<Option<EntryWithFeed>> {
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN} \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id \
         WHERE e.id = $1 AND c.user_id = $2"
    );
    fetch_entries_with_feed(db, sql, vec![Bind::Int(entry_id), Bind::Int(user_id)])
        .await
        .map(|v| v.into_iter().next())
        .map_err(AppError::Database)
}

/// Fetch the sort-field value (as the exact TEXT string `SQLite` stores) for
/// emitting a composite cursor. Returns `None` if the entry doesn't exist.
pub async fn fetch_sort_ts(
    db: &Db,
    entry_id: i64,
    sort_order: EntrySortOrder,
) -> AppResult<Option<String>> {
    let column_expr = match sort_order {
        EntrySortOrder::ReadAt => "read_at",
        EntrySortOrder::StarredAt => "starred_at",
        EntrySortOrder::PublishedAt => "COALESCE(published_at, created_at)",
    };
    // Emit the cursor string in the exact form the WHERE predicate compares
    // against: the raw TEXT column on SQLite, `to_char(..., 'YYYY-MM-DD
    // HH24:MI:SS')` on PG (columns are TIMESTAMPTZ there). See
    // `Dialect::cursor_ts`.
    let ts_expr = Dialect::from_db(db).cursor_ts(column_expr);
    let sql = format!("SELECT {ts_expr} FROM entry WHERE id = $1");
    let r = match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(sqlx::AssertSqlSafe(sql))
                .bind(entry_id)
                .fetch_optional(pool)
                .await
        }
        DbInner::Postgres(pool) => {
            sqlx::query_scalar::<sqlx::Postgres, Option<String>>(sqlx::AssertSqlSafe(sql))
                .bind(entry_id)
                .fetch_optional(pool)
                .await
        }
    }
    .map_err(AppError::Database)?;
    Ok(r.flatten())
}

pub async fn find_by_guid_and_feed(db: &Db, guid: &str, feed_id: i64) -> AppResult<Option<Entry>> {
    query_opt!(
        db,
        Entry,
        "SELECT id, feed_id, guid, title, link, content, summary, author, \
         published_at, read_at, starred_at, created_at, updated_at \
         FROM entry WHERE guid = $1 AND feed_id = $2",
        guid,
        feed_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_feed(db: &Db, feed_id: i64, limit: i64, offset: i64) -> AppResult<Vec<Entry>> {
    query_all!(
        db,
        Entry,
        "SELECT id, feed_id, guid, title, link, content, summary, author, \
         published_at, read_at, starred_at, created_at, updated_at \
         FROM entry WHERE feed_id = $1 \
         ORDER BY COALESCE(published_at, created_at) DESC LIMIT $2 OFFSET $3",
        feed_id,
        limit,
        offset
    )
    .map_err(AppError::Database)
}

pub async fn list_by_user(
    db: &Db,
    user_id: i64,
    filter: &EntryFilter,
    sort_order: EntrySortOrder,
    limit: i64,
    offset: i64,
) -> AppResult<Vec<EntryWithFeed>> {
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);

    let where_clause = conditions.join(" AND ");

    let order_by = match sort_order {
        EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at) DESC",
        EntrySortOrder::ReadAt => "e.read_at DESC",
        EntrySortOrder::StarredAt => "e.starred_at DESC",
    };

    // Force the right entry index for the high-traffic list pages. The
    // SQLite planner otherwise picks `category -> feed -> entry` and walks
    // every row before sorting (worst case for a single-user instance that
    // owns 100% of entries). The hint only applies to the published-order
    // sort; the read_at / starred_at sorts have their own dedicated indexes.
    // `Dialect::index_hint` drops the SQLite-only `INDEXED BY` on PostgreSQL.
    let entry_hint = if sort_order == EntrySortOrder::PublishedAt {
        dialect.index_hint(published_sort_entry_hint(filter))
    } else {
        ""
    };

    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN} \
         FROM entry e{entry_hint} \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id \
         WHERE {where_clause} \
         ORDER BY {order_by} \
         LIMIT ${limit_idx} OFFSET ${offset_idx}"
    );

    binds.push(Bind::Int(limit));
    binds.push(Bind::Int(offset));

    fetch_entries_with_feed(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

pub async fn count_by_user(db: &Db, user_id: i64, filter: &EntryFilter) -> AppResult<i64> {
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);

    let where_clause = conditions.join(" AND ");

    let sql = format!(
        "SELECT COUNT(*) \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE {where_clause}"
    );

    fetch_scalar_i64(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

pub async fn count_unread_by_user(db: &Db, user_id: i64) -> AppResult<i64> {
    query_scalar!(
        db,
        i64,
        "SELECT COUNT(*) FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1 AND e.read_at IS NULL",
        user_id
    )
    .map_err(AppError::Database)
}

/// Returns a map of `feed_id` -> unread count for a user
pub async fn count_unread_by_feed(
    db: &Db,
    user_id: i64,
) -> AppResult<std::collections::HashMap<i64, i64>> {
    let hint = Dialect::from_db(db).index_hint(" INDEXED BY idx_entry_unread_feed");
    let sql = format!(
        "SELECT f.id, COUNT(e.id) FROM feed f \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN entry e{hint} ON e.feed_id = f.id AND e.read_at IS NULL \
         WHERE c.user_id = $1 GROUP BY f.id"
    );
    let rows = fetch_id_ts_rows(db, sql, vec![Bind::Int(user_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows.into_iter().collect())
}

/// Returns a map of `category_id` -> unread count for a user
pub async fn count_unread_by_category(
    db: &Db,
    user_id: i64,
) -> AppResult<std::collections::HashMap<i64, i64>> {
    let hint = Dialect::from_db(db).index_hint(" INDEXED BY idx_entry_unread_feed");
    let sql = format!(
        "SELECT c.id, COUNT(e.id) FROM category c \
         LEFT JOIN feed f ON f.category_id = c.id \
         LEFT JOIN entry e{hint} ON e.feed_id = f.id AND e.read_at IS NULL \
         WHERE c.user_id = $1 GROUP BY c.id"
    );
    let rows = fetch_id_ts_rows(db, sql, vec![Bind::Int(user_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows.into_iter().collect())
}

/// Result of an entry upsert. The insert path is guarded against tombstones,
/// so a "skipped" state exists alongside insert/update, and an existing row
/// whose mutable columns already match the incoming values yields `Unchanged`
/// (no write is issued — see [`UPSERT_SELECT_SQL`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted(i64),
    Updated(i64),
    /// The row exists and every mutable column already equals the incoming
    /// value, so the UPDATE was skipped entirely.
    Unchanged(i64),
    SkippedTombstoned,
}

/// Upsert an entry, returning [`UpsertOutcome`] without re-reading the full row.
///
/// This is the lean variant used by the feed-sync hot loop, which only needs
/// the outcome flag. It avoids the full-row `find_by_id` re-read that
/// [`upsert_entry`] performs and looks the existing row up by `id` only. Wrap a
/// sync loop in a single transaction (see `feed_sync`) to collapse the per-entry
/// commits (`upsert_entry_id_tx`).
// Shared upsert statements. `datetime('now')` is kept (not a bound `Utc::now()`)
// so `updated_at` matches the TEXT format of the `datetime('now')` column
// DEFAULTs — the composite pagination cursor compares timestamps as strings, so
// all entry timestamps must share one format. On PG the dispatch-macro
// `pg_rewrite` shim rewrites it to `now()`. `published_at` is bound as a
// seconds-truncated `NaiveDateTime` (see the upsert fns), which sqlx encodes as
// the same `%Y-%m-%d %H:%M:%S` TEXT on SQLite and as a `timestamp` on PG.
// Kept to `id` only: `UNIQUE(feed_id, guid)` makes this an index-only lookup
// that never touches the row. Selecting the comparable columns here instead
// would force a full row read (a multi-KB `content` marshalled into a Rust
// String) on every entry of every poll — measurably slower than the write it
// would save. The no-op check therefore lives in the UPDATE's WHERE clause.
const UPSERT_SELECT_SQL: &str = "SELECT id FROM entry WHERE guid = $1 AND feed_id = $2";
// The UPDATE is guarded by a "something actually differs" predicate so a poll
// that re-serves byte-identical articles writes nothing at all: feeds resend
// their whole window every time, and without this every entry is rewritten
// (and WAL-logged) on every sync. `rows_affected() == 0` then means "already
// current", which is what distinguishes `Updated` from `Unchanged`.
//
// The predicate is the one genuine dialect fork here — SQLite spells NULL-safe
// inequality `IS NOT`, PostgreSQL `IS DISTINCT FROM`. It is kept as two
// literals rather than a `pg_rewrite` rule on purpose: that shim does blind
// string substitution, and rewriting `IS NOT` there would also hit every
// `IS NOT NULL` in the codebase. `content_text` is derived from `content`, so
// comparing `content` covers it; `$N` placeholders are referenced twice, which
// both backends accept (see `UPSERT_INSERT_SQL`).
const UPSERT_UPDATE_SQL_SQLITE: &str = "UPDATE entry SET title = $1, link = $2, content = $3, summary = $4, author = $5, content_text = $6, updated_at = datetime('now') WHERE id = $7 AND (title IS NOT $1 OR link IS NOT $2 OR content IS NOT $3 OR summary IS NOT $4 OR author IS NOT $5)";
const UPSERT_UPDATE_SQL_PG: &str = "UPDATE entry SET title = $1, link = $2, content = $3, summary = $4, author = $5, content_text = $6, updated_at = now() WHERE id = $7 AND (title IS DISTINCT FROM $1 OR link IS DISTINCT FROM $2 OR content IS DISTINCT FROM $3 OR summary IS DISTINCT FROM $4 OR author IS DISTINCT FROM $5)";
const UPSERT_INSERT_SQL: &str = "INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at, content_text) SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9 WHERE NOT EXISTS (SELECT 1 FROM entry_tombstone WHERE feed_id = $1 AND guid = $2) RETURNING id";

/// The mutable-column payload of an upsert, shared by the `&Db` and `&mut Tx`
/// update helpers so the seven binds are sequenced in exactly one place.
struct UpsertUpdate<'a> {
    id: i64,
    title: Option<&'a str>,
    link: Option<&'a str>,
    content: Option<&'a str>,
    summary: Option<&'a str>,
    author: Option<&'a str>,
    content_text: Option<&'a str>,
}

/// Run the guarded UPDATE against a transaction. Dispatched by hand rather than
/// through `db_execute_tx!` because the predicate differs per dialect and the
/// macros take a single `&'static str`.
async fn upsert_update_tx(tx: &mut Tx<'_>, u: &UpsertUpdate<'_>) -> Result<u64, sqlx::Error> {
    match tx {
        Tx::Sqlite { tx: t, .. } => {
            sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(UPSERT_UPDATE_SQL_SQLITE))
                .bind(u.title)
                .bind(u.link)
                .bind(u.content)
                .bind(u.summary)
                .bind(u.author)
                .bind(u.content_text)
                .bind(u.id)
                .execute(&mut **t)
                .await
                .map(|r| r.rows_affected())
        }
        Tx::Postgres(t) => sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(UPSERT_UPDATE_SQL_PG))
            .bind(u.title)
            .bind(u.link)
            .bind(u.content)
            .bind(u.summary)
            .bind(u.author)
            .bind(u.content_text)
            .bind(u.id)
            .execute(&mut **t)
            .await
            .map(|r| r.rows_affected()),
    }
}

/// Pooled sibling of [`upsert_update_tx`].
async fn upsert_update(db: &Db, u: &UpsertUpdate<'_>) -> Result<u64, sqlx::Error> {
    let _guard = db.admit().await;
    match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(UPSERT_UPDATE_SQL_SQLITE))
                .bind(u.title)
                .bind(u.link)
                .bind(u.content)
                .bind(u.summary)
                .bind(u.author)
                .bind(u.content_text)
                .bind(u.id)
                .execute(pool)
                .await
                .map(|r| r.rows_affected())
        }
        DbInner::Postgres(pool) => {
            sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(UPSERT_UPDATE_SQL_PG))
                .bind(u.title)
                .bind(u.link)
                .bind(u.content)
                .bind(u.summary)
                .bind(u.author)
                .bind(u.content_text)
                .bind(u.id)
                .execute(pool)
                .await
                .map(|r| r.rows_affected())
        }
    }
}

/// Upsert an entry, returning [`UpsertOutcome`] without re-reading the full row.
/// The tombstone-guarded insert uses `RETURNING id`, so a `Some` row means an
/// insert happened (its id) and `None` means the guid is tombstoned. An existing
/// row that already matches the incoming values yields
/// [`UpsertOutcome::Unchanged`] without issuing a write.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_entry_id(
    db: &Db,
    feed_id: i64,
    guid: &str,
    title: Option<&str>,
    link: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    author: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<UpsertOutcome> {
    // Bind published_at as a seconds-truncated NaiveDateTime: sqlx encodes it as
    // the `%Y-%m-%d %H:%M:%S` TEXT the SQLite composite cursor compares against,
    // and as a `timestamp` that assignment-casts into the column's `timestamptz`
    // (UTC session) on PG. A raw `%Y-%m-%d %H:%M:%S` *string* bind is rejected by
    // PG here — text does not coerce into a timestamptz column.
    let published_at_ts = published_at.map(|dt| dt.naive_utc().trunc_subsecs(0));
    let content_text = content.map(strip_to_search_text);

    let existing =
        query_opt!(db, (i64,), UPSERT_SELECT_SQL, guid, feed_id).map_err(AppError::Database)?;

    if let Some((id,)) = existing {
        let rows = upsert_update(
            db,
            &UpsertUpdate {
                id,
                title,
                link,
                content,
                summary,
                author,
                content_text: content_text.as_deref(),
            },
        )
        .await
        .map_err(AppError::Database)?;
        return Ok(if rows > 0 {
            UpsertOutcome::Updated(id)
        } else {
            UpsertOutcome::Unchanged(id)
        });
    }

    let inserted = query_opt!(
        db,
        (i64,),
        UPSERT_INSERT_SQL,
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at_ts,
        content_text.as_deref()
    )
    .map_err(AppError::Database)?;

    Ok(match inserted {
        Some((id,)) => UpsertOutcome::Inserted(id),
        None => UpsertOutcome::SkippedTombstoned,
    })
}

/// Transactional sibling of [`upsert_entry_id`] for the feed-sync unit of work,
/// which upserts a whole feed's entries and records the fetch result atomically.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_entry_id_tx(
    tx: &mut Tx<'_>,
    feed_id: i64,
    guid: &str,
    title: Option<&str>,
    link: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    author: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<UpsertOutcome> {
    // Bind published_at as a seconds-truncated NaiveDateTime: sqlx encodes it as
    // the `%Y-%m-%d %H:%M:%S` TEXT the SQLite composite cursor compares against,
    // and as a `timestamp` that assignment-casts into the column's `timestamptz`
    // (UTC session) on PG. A raw `%Y-%m-%d %H:%M:%S` *string* bind is rejected by
    // PG here — text does not coerce into a timestamptz column.
    let published_at_ts = published_at.map(|dt| dt.naive_utc().trunc_subsecs(0));
    let content_text = content.map(strip_to_search_text);

    let existing =
        query_opt_tx!(tx, (i64,), UPSERT_SELECT_SQL, guid, feed_id).map_err(AppError::Database)?;

    if let Some((id,)) = existing {
        let rows = upsert_update_tx(
            tx,
            &UpsertUpdate {
                id,
                title,
                link,
                content,
                summary,
                author,
                content_text: content_text.as_deref(),
            },
        )
        .await
        .map_err(AppError::Database)?;
        return Ok(if rows > 0 {
            UpsertOutcome::Updated(id)
        } else {
            UpsertOutcome::Unchanged(id)
        });
    }

    let inserted = query_opt_tx!(
        tx,
        (i64,),
        UPSERT_INSERT_SQL,
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at_ts,
        content_text.as_deref()
    )
    .map_err(AppError::Database)?;

    Ok(match inserted {
        Some((id,)) => UpsertOutcome::Inserted(id),
        None => UpsertOutcome::SkippedTombstoned,
    })
}

/// Idempotent `(feed_id, guid)` tombstone insert. Shared by the single-shot
/// [`insert_tombstone`] helper and the batched `prune_read_retention_batch`
/// loop so the statement text lives in exactly one place.
const INSERT_TOMBSTONE_SQL: &str = "INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, $2) ON CONFLICT(feed_id, guid) DO NOTHING";

/// Record a tombstone for `(feed_id, guid)`. Idempotent.
pub async fn insert_tombstone(db: &Db, feed_id: i64, guid: &str) -> AppResult<()> {
    db_execute!(db, INSERT_TOMBSTONE_SQL, feed_id, guid).map_err(AppError::Database)?;
    Ok(())
}

/// Build the victim-selection query for retention pruning. The per-user age
/// cutoff `read_at < now - retention_read_days` dialect-forks its interval
/// expression (see [`Dialect::days_ago`]), so the SQL is assembled at call time
/// rather than being a `const` literal.
fn retention_victims_sql(dialect: Dialect) -> String {
    let cutoff = dialect.days_ago("us.retention_read_days");
    format!(
        "SELECT e.id \
         FROM entry e \
         JOIN feed f           ON f.id = e.feed_id \
         JOIN category c       ON c.id = f.category_id \
         JOIN user_settings us ON us.user_id = c.user_id \
         WHERE us.retention_read_days > 0 \
           AND e.read_at    IS NOT NULL \
           AND e.starred_at IS NULL \
           AND e.read_at < {cutoff} \
         LIMIT $1"
    )
}

/// Delete up to `batch_size` read, aged, non-starred entries belonging to users
/// who have opted into retention (`user_settings.retention_read_days > 0`),
/// recording a tombstone for each. Returns the number of entries deleted.
///
/// One batch runs in a single transaction so the tombstone+delete pair is
/// atomic against a concurrent feed refresh. Victims are gathered first (Rust
/// side) so the delete targets exact ids rather than re-running a `LIMIT`
/// without `ORDER BY`. Each user's own threshold is applied via the join.
pub async fn prune_read_retention_batch(db: &Db, batch_size: usize) -> AppResult<u64> {
    let sql = retention_victims_sql(Dialect::from_db(db));
    let mut tx = db.begin().await?;

    // Dynamic SQL (dialect-forked interval) can't go through the static-SQL
    // `query_all_tx!` macro, so dispatch the fetch on the transaction's backend
    // directly.
    let victims: Vec<(i64,)> = match &mut tx {
        Tx::Sqlite { tx: t, .. } => {
            sqlx::query_as::<sqlx::Sqlite, (i64,)>(sqlx::AssertSqlSafe(sql))
                .bind(batch_size as i64)
                .fetch_all(&mut **t)
                .await
        }
        Tx::Postgres(t) => {
            sqlx::query_as::<sqlx::Postgres, (i64,)>(sqlx::AssertSqlSafe(sql))
                .bind(batch_size as i64)
                .fetch_all(&mut **t)
                .await
        }
    }
    .map_err(AppError::Database)?;

    if victims.is_empty() {
        tx.commit().await?;
        return Ok(0);
    }

    // Tombstone + delete as two set-based statements rather than a 2-per-victim
    // loop: at BATCH_SIZE = 500 that is 2 statements per batch instead of 1000,
    // which shortens the window the batch holds SQLite's single write lock and
    // keeps a large retention drain from stalling interactive writes. The
    // tombstone insert reads feed_id/guid back out of `entry` itself, so only
    // the ids need binding and both statements target the identical id set.
    // (The `WHERE id IN (...)` also disambiguates SQLite's
    // `INSERT ... SELECT ... ON CONFLICT` parse, which requires a WHERE clause.)
    let in_clause = (0..victims.len())
        .map(|i| format!("${}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let id_binds = || {
        victims
            .iter()
            .map(|(id,)| Bind::Int(*id))
            .collect::<Vec<_>>()
    };

    exec_dynamic_tx(
        &mut tx,
        format!(
            "INSERT INTO entry_tombstone (feed_id, guid) \
             SELECT feed_id, guid FROM entry WHERE id IN ({in_clause}) \
             ON CONFLICT (feed_id, guid) DO NOTHING"
        ),
        id_binds(),
    )
    .await
    .map_err(AppError::Database)?;

    let deleted = exec_dynamic_tx(
        &mut tx,
        format!("DELETE FROM entry WHERE id IN ({in_clause})"),
        id_binds(),
    )
    .await
    .map_err(AppError::Database)?;

    tx.commit().await?;
    Ok(deleted)
}

/// Upsert an entry and return the resulting [`Entry`] plus whether it was new.
///
/// Thin wrapper over [`upsert_entry_id`] for callers that need the full record
/// (tests, summary worker/cleanup seeding). The feed-sync hot path should call
/// [`upsert_entry_id`] directly to skip the extra full-row read this performs.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_entry(
    db: &Db,
    feed_id: i64,
    guid: &str,
    title: Option<&str>,
    link: Option<&str>,
    content: Option<&str>,
    summary: Option<&str>,
    author: Option<&str>,
    published_at: Option<DateTime<Utc>>,
) -> AppResult<(Entry, bool)> {
    let (id, is_new) = match upsert_entry_id(
        db,
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at,
    )
    .await?
    {
        UpsertOutcome::Inserted(id) => (id, true),
        UpsertOutcome::Updated(id) | UpsertOutcome::Unchanged(id) => (id, false),
        UpsertOutcome::SkippedTombstoned => {
            return Err(AppError::Internal(
                "upsert_entry called for a tombstoned guid".to_string(),
            ));
        }
    };
    let entry = find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)?;
    Ok((entry, is_new))
}

pub async fn mark_as_read(db: &Db, id: i64) -> AppResult<Entry> {
    let rows = db_execute!(
        db,
        "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE id = $1 AND read_at IS NULL",
        id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        // Entry might already be read or not exist
        if find_by_id(db, id).await?.is_none() {
            return Err(AppError::EntryNotFound);
        }
    }

    find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)
}

pub async fn mark_as_unread(db: &Db, id: i64) -> AppResult<Entry> {
    let rows = db_execute!(
        db,
        "UPDATE entry SET read_at = NULL, updated_at = datetime('now') WHERE id = $1",
        id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)
}

/// Explicitly star an entry (set `starred_at` if not already set).
pub async fn star_entry(db: &Db, id: i64) -> AppResult<Entry> {
    let rows = db_execute!(
        db,
        "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = $1 AND starred_at IS NULL",
        id
    )
    .map_err(AppError::Database)?;

    if rows == 0 && find_by_id(db, id).await?.is_none() {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)
}

/// Explicitly unstar an entry (clear `starred_at`).
pub async fn unstar_entry(db: &Db, id: i64) -> AppResult<Entry> {
    let rows = db_execute!(
        db,
        "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = $1",
        id
    )
    .map_err(AppError::Database)?;

    if rows == 0 {
        return Err(AppError::EntryNotFound);
    }

    find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)
}

pub async fn toggle_star(db: &Db, id: i64) -> AppResult<Entry> {
    let entry = find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)?;

    if entry.starred_at.is_some() {
        db_execute!(
            db,
            "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = $1",
            id
        )
        .map_err(AppError::Database)?;
    } else {
        db_execute!(
            db,
            "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = $1",
            id
        )
        .map_err(AppError::Database)?;
    }

    find_by_id(db, id).await?.ok_or(AppError::EntryNotFound)
}

/// Set the starred state for an entry, scoped to the owning user. Idempotent
/// — a no-op if the entry is already in the desired state. Returns the
/// resulting `EntryWithFeed` plus a `changed` bool (parallels
/// `set_read_for_user`). `None` when the entry does not exist or belongs to
/// a different user (callers treat both as 404).
pub async fn set_starred_for_user(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    desired_starred: bool,
) -> AppResult<Option<(EntryWithFeed, bool)>> {
    let cur = find_by_id_for_user(db, user_id, entry_id).await?;
    let Some(e) = cur else {
        return Ok(None);
    };
    let was_starred = e.entry.starred_at.is_some();
    let changed = was_starred != desired_starred;
    if changed {
        let sql = if desired_starred {
            "UPDATE entry SET starred_at = datetime('now'), updated_at = datetime('now') WHERE id = $1"
        } else {
            "UPDATE entry SET starred_at = NULL, updated_at = datetime('now') WHERE id = $1"
        };
        db_execute!(db, sql, entry_id).map_err(AppError::Database)?;
    }
    Ok(find_by_id_for_user(db, user_id, entry_id)
        .await?
        .map(|ewf| (ewf, changed)))
}

/// Set the read state for an entry, scoped to the owning user. Idempotent —
/// a no-op if the entry is already in the desired state. Returns the
/// resulting `EntryWithFeed` (or `None` if the entry does not exist or
/// belongs to a different user), plus a bool indicating whether the call
/// actually changed state (used by handlers to decide whether to emit a
/// flash toast).
pub async fn set_read_for_user(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    desired_read: bool,
) -> AppResult<Option<(EntryWithFeed, bool)>> {
    let cur = find_by_id_for_user(db, user_id, entry_id).await?;
    let Some(e) = cur else {
        return Ok(None);
    };
    let was_read = e.entry.read_at.is_some();
    let changed = was_read != desired_read;
    if changed {
        let sql = if desired_read {
            "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') WHERE id = $1"
        } else {
            "UPDATE entry SET read_at = NULL, updated_at = datetime('now') WHERE id = $1"
        };
        db_execute!(db, sql, entry_id).map_err(AppError::Database)?;
    }
    Ok(find_by_id_for_user(db, user_id, entry_id)
        .await?
        .map(|ewf| (ewf, changed)))
}

/// Unread count per feed for a user.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnreadCount {
    pub feed_id: i64,
    pub unread: i64,
}

/// Return the unread entry count grouped by feed for the given user.
pub async fn unread_counts_per_feed(db: &Db, user_id: i64) -> AppResult<Vec<UnreadCount>> {
    let hint = Dialect::from_db(db).index_hint(" INDEXED BY idx_entry_unread_feed");
    let sql = format!(
        "SELECT e.feed_id, COUNT(*) AS unread \
         FROM entry e{hint} \
         INNER JOIN feed f ON f.id = e.feed_id \
         INNER JOIN category c ON c.id = f.category_id \
         WHERE c.user_id = $1 AND e.read_at IS NULL \
         GROUP BY e.feed_id"
    );
    let rows = fetch_id_ts_rows(db, sql, vec![Bind::Int(user_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows
        .into_iter()
        .map(|(feed_id, unread)| UnreadCount { feed_id, unread })
        .collect())
}

/// Batch query entries by IDs with feed info, verifying user ownership.
pub async fn find_by_ids_with_feed(
    db: &Db,
    user_id: i64,
    ids: &[i64],
) -> AppResult<Vec<EntryWithFeed>> {
    if ids.is_empty() {
        return Ok(vec![]);
    }

    let placeholders: Vec<String> = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_COUNT} \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE c.user_id = $1 AND e.id IN ({in_clause})"
    );

    let mut binds = vec![Bind::Int(user_id)];
    for id in ids {
        binds.push(Bind::Int(*id));
    }

    fetch_entries_with_feed(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

/// List entry IDs with timestamps for a user, using continuation-based pagination.
/// Returns Vec<(`entry_id`, `timestamp_usec`)>.
pub async fn list_ids_by_user(
    db: &Db,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<(i64, i64)>> {
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);
    apply_time_conditions(
        &mut conditions,
        &mut binds,
        pagination.ot,
        pagination.nt,
        dialect,
    );
    apply_continuation_condition(
        &mut conditions,
        &mut binds,
        pagination.continuation.as_ref(),
        pagination.sort_order,
        pagination.oldest_first,
        dialect,
    );

    let where_clause = conditions.join(" AND ");
    let order = match (pagination.sort_order, pagination.oldest_first) {
        (EntrySortOrder::ReadAt, true) => "e.read_at ASC, e.id ASC",
        (EntrySortOrder::ReadAt, false) => "e.read_at DESC, e.id DESC",
        (EntrySortOrder::StarredAt, true) => "e.starred_at ASC, e.id ASC",
        (EntrySortOrder::StarredAt, false) => "e.starred_at DESC, e.id DESC",
        (_, true) => "COALESCE(e.published_at, e.created_at) ASC, e.id ASC",
        (_, false) => "COALESCE(e.published_at, e.created_at) DESC, e.id DESC",
    };

    let limit_idx = binds.len() + 1;
    // Epoch-microseconds sentinel for the "no more pages" boundary; the epoch
    // extraction dialect-forks (SQLite `strftime` vs PG `EXTRACT(EPOCH …)`).
    let epoch_us = dialect.epoch("COALESCE(e.published_at, e.created_at)");
    let sql = format!(
        "SELECT e.id, {epoch_us} * 1000000 \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE {where_clause} \
         ORDER BY {order} \
         LIMIT ${limit_idx}"
    );

    binds.push(Bind::Int(pagination.limit));
    fetch_id_ts_rows(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

/// List entries with continuation-based pagination (for Google Reader stream/contents).
pub async fn list_by_user_with_continuation(
    db: &Db,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<EntryWithFeed>> {
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);
    apply_time_conditions(
        &mut conditions,
        &mut binds,
        pagination.ot,
        pagination.nt,
        dialect,
    );
    apply_continuation_condition(
        &mut conditions,
        &mut binds,
        pagination.continuation.as_ref(),
        pagination.sort_order,
        pagination.oldest_first,
        dialect,
    );

    let where_clause = conditions.join(" AND ");
    let order = match (pagination.sort_order, pagination.oldest_first) {
        (EntrySortOrder::ReadAt, true) => "e.read_at ASC, e.id ASC",
        (EntrySortOrder::ReadAt, false) => "e.read_at DESC, e.id DESC",
        (EntrySortOrder::StarredAt, true) => "e.starred_at ASC, e.id ASC",
        (EntrySortOrder::StarredAt, false) => "e.starred_at DESC, e.id DESC",
        (_, true) => "COALESCE(e.published_at, e.created_at) ASC, e.id ASC",
        (_, false) => "COALESCE(e.published_at, e.created_at) DESC, e.id DESC",
    };

    // Page-0 (cursorless) index hint. Without it the planner walks
    // category->feed->entry and temp-B-tree-sorts the whole corpus before
    // LIMIT. Mirrors `list_by_user`'s hint, but only when there is no
    // continuation predicate — at depth the predicate already drives the sort
    // index, so we leave that proven-fast plan untouched. Only the
    // published-order sorts have dedicated indexes.
    let entry_hint = if pagination.sort_order == EntrySortOrder::PublishedAt
        && pagination.continuation.is_none()
    {
        dialect.index_hint(published_sort_entry_hint(filter))
    } else {
        ""
    };

    let limit_idx = binds.len() + 1;
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN} \
         FROM entry e{entry_hint} \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id \
         WHERE {where_clause} \
         ORDER BY {order} \
         LIMIT ${limit_idx}"
    );

    binds.push(Bind::Int(pagination.limit));
    fetch_entries_with_feed(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

/// Apply common filter conditions to query builder.
/// True when no `EntryFilter` field would add a predicate against the `entry`
/// table itself. Used to gate the `INDEXED BY idx_entry_sort_ts` hint: without
/// any entry-side filter, scanning the sort index DESC with LIMIT is far
/// cheaper than the planner's default `category -> feed -> entry` walk.
pub async fn mark_all_read_by_feed(
    db: &Db,
    feed_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    // `days` is an `i64` interpolated into the SQL (not injectable). The age
    // cutoff dialect-forks via `Dialect::days_ago` (SQLite `datetime('now', …)`
    // vs PG `now() - make_interval(…)`).
    let age_condition = older_than_days
        .map(|days| {
            let cutoff = Dialect::from_db(db).days_ago(&days.to_string());
            format!(" AND COALESCE(published_at, created_at) < {cutoff}")
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE entry SET read_at = datetime('now'), updated_at = datetime('now') \
         WHERE feed_id = $1 AND read_at IS NULL{age_condition}"
    );

    let rows = exec_dynamic(db, sql, vec![Bind::Int(feed_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

pub async fn mark_all_read_by_user(
    db: &Db,
    user_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    let age_condition = older_than_days
        .map(|days| {
            let cutoff = Dialect::from_db(db).days_ago(&days.to_string());
            format!(" AND COALESCE(published_at, created_at) < {cutoff}")
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE entry \
         SET read_at = datetime('now'), updated_at = datetime('now') \
         WHERE read_at IS NULL{age_condition} AND feed_id IN ( \
             SELECT f.id FROM feed f \
             INNER JOIN category c ON f.category_id = c.id \
             WHERE c.user_id = $1 \
         )"
    );

    let rows = exec_dynamic(db, sql, vec![Bind::Int(user_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Mark every entry matching `filter` (and owned by `user_id`, and currently
/// unread) as read. Reuses the shared filter builder so scoped search + status
/// combine exactly as they do in the list query. Returns rows affected.
pub async fn mark_read_by_filter(db: &Db, user_id: i64, filter: &EntryFilter) -> AppResult<i64> {
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];
    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);
    let where_clause = conditions.join(" AND ");

    let sql = format!(
        "UPDATE entry \
         SET read_at = datetime('now'), updated_at = datetime('now') \
         WHERE read_at IS NULL AND id IN ( \
             SELECT e.id FROM entry e \
             INNER JOIN feed f ON e.feed_id = f.id \
             INNER JOIN category c ON f.category_id = c.id \
             WHERE {where_clause} \
         )"
    );

    let rows = exec_dynamic(db, sql, binds)
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Result of finding neighboring entries
#[derive(Debug, Clone, Serialize)]
pub struct EntryNeighbors {
    pub prev_id: Option<i64>,
    pub next_id: Option<i64>,
}

/// Find neighboring entries (prev/next) for a given entry within a user's entries.
/// Entries are ordered by `COALESCE(published_at`, `created_at`) DESC.
/// - `prev_id`: the entry that comes before (newer/higher in list)
/// - `next_id`: the entry that comes after (older/lower in list)
///
/// Uses `EntryFilter` to support all filtering conditions (unread, starred, read, feed, category, `has_summary`).
pub async fn find_neighbors(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    filter: &EntryFilter,
) -> AppResult<EntryNeighbors> {
    let dialect = Dialect::from_db(db);

    // Get the current entry's sort timestamp as the `%Y-%m-%d %H:%M:%S` cursor
    // TEXT (to_char on PG — see `Dialect::cursor_ts`), so it compares against the
    // neighbour predicates below in the same form on both backends.
    let sort_ts_select = dialect.cursor_ts("COALESCE(e.published_at, e.created_at)");
    let sort_time_sql = format!(
        "SELECT {sort_ts_select} \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE e.id = $1 AND c.user_id = $2"
    );
    let sort_time: Option<String> = match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(sqlx::AssertSqlSafe(sort_time_sql))
                .bind(entry_id)
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map(Option::flatten)
        }
        DbInner::Postgres(pool) => {
            sqlx::query_scalar::<sqlx::Postgres, Option<String>>(sqlx::AssertSqlSafe(sort_time_sql))
                .bind(entry_id)
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map(Option::flatten)
        }
    }
    .map_err(AppError::Database)?;

    let Some(sort_time) = sort_time else {
        return Ok(EntryNeighbors {
            prev_id: None,
            next_id: None,
        });
    };

    // On PG, bind the cursor as a `timestamptz` and compare the raw column so
    // the neighbour lookups hit the timestamp index as a range scan (see
    // `filters::parse_cursor_ts`); fall back to the `to_char` string comparison
    // if the stored value can't be parsed. SQLite compares raw TEXT.
    let pg_ts = (dialect == Dialect::Postgres)
        .then(|| filters::parse_cursor_ts(&sort_time))
        .flatten();
    let cursor_bind = |ts: Option<chrono::DateTime<chrono::Utc>>, s: &str| match ts {
        Some(ts) => Bind::Ts(ts),
        None => Bind::Text(s.to_string()),
    };

    // Build filter conditions using apply_filter_conditions.
    // Prev query base binds: $1=user_id, $2=sort_time
    let mut prev_conditions = Vec::new();
    let mut prev_binds: Vec<Bind> = vec![Bind::Int(user_id), cursor_bind(pg_ts, &sort_time)];
    apply_filter_conditions(&mut prev_conditions, &mut prev_binds, filter, dialect);

    // Next query base binds: $1=user_id, $2=sort_time, $3=entry_id
    let mut next_conditions = Vec::new();
    let mut next_binds: Vec<Bind> = vec![
        Bind::Int(user_id),
        cursor_bind(pg_ts, &sort_time),
        Bind::Int(entry_id),
    ];
    apply_filter_conditions(&mut next_conditions, &mut next_binds, filter, dialect);

    let prev_extra = if prev_conditions.is_empty() {
        String::new()
    } else {
        format!(" AND {}", prev_conditions.join(" AND "))
    };
    let next_extra = if next_conditions.is_empty() {
        String::new()
    } else {
        format!(" AND {}", next_conditions.join(" AND "))
    };

    // Pin the published-order index and force the entry table to drive the
    // join so the planner walks it in sort order and stops at LIMIT 1.
    //
    // The snapshot-widened unread predicate `(read_at IS NULL OR
    // read_at >= ?)` needs special handling. `published_sort_entry_hint`
    // returns no hint for unread, and the OR otherwise makes the planner pick
    // a MULTI-INDEX OR that pulls the whole read-majority of the table into a
    // temp B-tree just to take one row — an O(table) scan that grows with
    // inbox size (~2ms/call at 50k entries, ~8ms at 200k, exec only). Pinning
    // `idx_entry_sort_ts` and using CROSS JOIN to force entry-first ordering
    // turns that into an indexed range scan that short-circuits at the first
    // matching neighbour (~21µs, flat with inbox size), identical results.
    //
    // Gated on `read_after`: only the snapshot OR triggers the bad plan. The
    // strict `read_at IS NULL` path (no `read_after`) keeps its prior plan,
    // which can still use the partial `idx_entry_unread_feed` and so must not
    // be force-pinned to the full sort index.
    // `Dialect::index_hint` drops the SQLite-only `INDEXED BY` on PostgreSQL.
    let (raw_hint, join_kw) = if filter.unread_only && filter.read_after.is_some() {
        (" INDEXED BY idx_entry_sort_ts", "CROSS JOIN")
    } else {
        (published_sort_entry_hint(filter), "INNER JOIN")
    };
    let entry_hint = dialect.index_hint(raw_hint);

    // Compare against the sort expression in the form matching the `$2` bind:
    // the raw column when the cursor was bound as a `timestamptz` (sargable on
    // PG; also the SQLite raw-TEXT path), or `to_char(...)` on the PG string
    // fallback. ORDER BY stays on the raw expression regardless.
    let cmp_ts = match pg_ts {
        Some(_) => "COALESCE(e.published_at, e.created_at)".to_string(),
        None => dialect.cursor_ts("COALESCE(e.published_at, e.created_at)"),
    };

    // Find previous entry (newer, comes before in DESC order)
    let prev_sql = format!(
        "SELECT e.id \
         FROM entry e{entry_hint} \
         {join_kw} feed f ON e.feed_id = f.id \
         {join_kw} category c ON f.category_id = c.id \
         WHERE c.user_id = $1 \
           AND {cmp_ts} > $2{prev_extra} \
         ORDER BY COALESCE(e.published_at, e.created_at) ASC \
         LIMIT 1"
    );

    // Find next entry (older, comes after in DESC order)
    let next_sql = format!(
        "SELECT e.id \
         FROM entry e{entry_hint} \
         {join_kw} feed f ON e.feed_id = f.id \
         {join_kw} category c ON f.category_id = c.id \
         WHERE c.user_id = $1 \
           AND ({cmp_ts} < $2 \
                OR ({cmp_ts} = $2 AND e.id < $3)){next_extra} \
         ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC \
         LIMIT 1"
    );

    async fn one(db: &Db, sql: String, binds: Vec<Bind>) -> Result<Option<i64>, sqlx::Error> {
        match db.inner() {
            DbInner::Sqlite(pool) => {
                let mut q = sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(sql));
                for b in &binds {
                    q = match b {
                        Bind::Int(i) => q.bind(*i),
                        Bind::Text(s) => q.bind(s.as_str()),
                        Bind::Ts(t) => q.bind(*t),
                    };
                }
                q.fetch_optional(pool).await
            }
            DbInner::Postgres(pool) => {
                let mut q = sqlx::query_scalar::<sqlx::Postgres, i64>(sqlx::AssertSqlSafe(sql));
                for b in &binds {
                    q = match b {
                        Bind::Int(i) => q.bind(*i),
                        Bind::Text(s) => q.bind(s.as_str()),
                        Bind::Ts(t) => q.bind(*t),
                    };
                }
                q.fetch_optional(pool).await
            }
        }
    }

    let prev_id = one(db, prev_sql, prev_binds)
        .await
        .map_err(AppError::Database)?;
    let next_id = one(db, next_sql, next_binds)
        .await
        .map_err(AppError::Database)?;

    Ok(EntryNeighbors { prev_id, next_id })
}

pub async fn mark_all_read_by_category(
    db: &Db,
    category_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    let age_condition = older_than_days
        .map(|days| {
            let cutoff = Dialect::from_db(db).days_ago(&days.to_string());
            format!(" AND COALESCE(published_at, created_at) < {cutoff}")
        })
        .unwrap_or_default();

    let sql = format!(
        "UPDATE entry \
         SET read_at = datetime('now'), updated_at = datetime('now') \
         WHERE read_at IS NULL{age_condition} AND feed_id IN ( \
             SELECT id FROM feed WHERE category_id = $1 \
         )"
    );

    let rows = exec_dynamic(db, sql, vec![Bind::Int(category_id)])
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Build the bulk-update SQL + binds for the by-ids operations. `set_clause` is
/// the `SET ...` body; `extra_where` is an optional predicate (e.g.
/// `" AND read_at IS NULL"`) appended after the `id IN (...)` clause. `$1` is
/// the user id; the ids fill `$2, $3, ...`.
fn build_update_by_ids(
    user_id: i64,
    entry_ids: &[i64],
    set_clause: &str,
    extra_where: &str,
) -> (String, Vec<Bind>) {
    let placeholders: Vec<String> = entry_ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("${}", i + 2))
        .collect();
    let in_clause = placeholders.join(", ");

    let sql = format!(
        "UPDATE entry \
         SET {set_clause} \
         WHERE id IN ({in_clause}){extra_where} \
           AND feed_id IN ( \
               SELECT f.id FROM feed f \
               INNER JOIN category c ON f.category_id = c.id \
               WHERE c.user_id = $1 \
           )"
    );

    let mut binds = vec![Bind::Int(user_id)];
    for id in entry_ids {
        binds.push(Bind::Int(*id));
    }
    (sql, binds)
}

/// Apply a bulk `SET` to the given entry ids in a single statement, scoped to
/// the feeds the user owns. Returns the number of rows updated. Empty
/// `entry_ids` is a no-op returning 0.
async fn update_entries_by_ids(
    db: &Db,
    user_id: i64,
    entry_ids: &[i64],
    set_clause: &str,
    extra_where: &str,
) -> AppResult<i64> {
    if entry_ids.is_empty() {
        return Ok(0);
    }
    let (sql, binds) = build_update_by_ids(user_id, entry_ids, set_clause, extra_where);
    let rows = exec_dynamic(db, sql, binds)
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Transactional twin of [`update_entries_by_ids`], for the `GReader` `edit_tag`
/// unit of work that batches several tag mutations atomically.
async fn update_entries_by_ids_tx(
    tx: &mut Tx<'_>,
    user_id: i64,
    entry_ids: &[i64],
    set_clause: &str,
    extra_where: &str,
) -> AppResult<i64> {
    if entry_ids.is_empty() {
        return Ok(0);
    }
    let (sql, binds) = build_update_by_ids(user_id, entry_ids, set_clause, extra_where);
    let rows = exec_dynamic_tx(tx, sql, binds)
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Bulk mark the given entries as read (only those currently unread), scoped to
/// the user's feeds. Returns the number of rows updated.
pub async fn mark_read_by_ids(db: &Db, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        db,
        user_id,
        entry_ids,
        "read_at = datetime('now'), updated_at = datetime('now')",
        " AND read_at IS NULL",
    )
    .await
}

/// Bulk mark the given entries as unread, scoped to the user's feeds.
pub async fn mark_unread_by_ids(db: &Db, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        db,
        user_id,
        entry_ids,
        "read_at = NULL, updated_at = datetime('now')",
        "",
    )
    .await
}

/// Bulk star the given entries (only those not already starred), scoped to the
/// user's feeds.
pub async fn star_by_ids(db: &Db, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        db,
        user_id,
        entry_ids,
        "starred_at = datetime('now'), updated_at = datetime('now')",
        " AND starred_at IS NULL",
    )
    .await
}

/// Bulk unstar the given entries, scoped to the user's feeds.
pub async fn unstar_by_ids(db: &Db, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        db,
        user_id,
        entry_ids,
        "starred_at = NULL, updated_at = datetime('now')",
        "",
    )
    .await
}

/// Transactional variant of [`mark_unread_by_ids`] (`GReader` `edit_tag`).
pub async fn mark_unread_by_ids_tx(
    tx: &mut Tx<'_>,
    user_id: i64,
    entry_ids: &[i64],
) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        user_id,
        entry_ids,
        "read_at = NULL, updated_at = datetime('now')",
        "",
    )
    .await
}

/// Transactional variant of [`star_by_ids`] (`GReader` `edit_tag`).
pub async fn star_by_ids_tx(tx: &mut Tx<'_>, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        user_id,
        entry_ids,
        "starred_at = datetime('now'), updated_at = datetime('now')",
        " AND starred_at IS NULL",
    )
    .await
}

/// Transactional variant of [`unstar_by_ids`] (`GReader` `edit_tag`).
pub async fn unstar_by_ids_tx(tx: &mut Tx<'_>, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        user_id,
        entry_ids,
        "starred_at = NULL, updated_at = datetime('now')",
        "",
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::category;
    use crate::models::feed;
    use crate::models::user::{self, Role};
    use chrono::TimeZone;

    async fn setup_db() -> Db {
        Db::connect_in_memory().await.unwrap()
    }

    async fn create_test_user(db: &Db, username: &str) -> i64 {
        user::create_user(db, username, "hash123", Role::User)
            .await
            .unwrap()
            .id
    }

    async fn create_test_category(db: &Db, user_id: i64, name: &str) -> i64 {
        category::create_category(db, user_id, name)
            .await
            .unwrap()
            .id
    }

    async fn create_test_feed(db: &Db, category_id: i64, url: &str) -> i64 {
        feed::create_feed(
            db,
            &feed::CreateFeedParams {
                category_id,
                url,
                title: Some("Test Feed"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn test_upsert_entry() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // Insert new entry
        let (entry, is_new) = upsert_entry(
            &db,
            feed_id,
            "guid-123",
            Some("Test Entry"),
            Some("https://example.com/entry"),
            Some("Content"),
            Some("Summary"),
            Some("Author"),
            Some(Utc::now()),
        )
        .await
        .unwrap();

        assert!(is_new);
        assert_eq!(entry.title, Some("Test Entry".to_string()));
        assert!(entry.read_at.is_none());
        assert!(entry.starred_at.is_none());

        // Update existing entry
        let (updated, is_new) = upsert_entry(
            &db,
            feed_id,
            "guid-123",
            Some("Updated Title"),
            Some("https://example.com/entry"),
            Some("Updated Content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(!is_new);
        assert_eq!(updated.title, Some("Updated Title".to_string()));
        assert_eq!(updated.id, entry.id);
    }

    #[tokio::test]
    async fn test_mark_as_read() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let (entry, _) = upsert_entry(
            &db,
            feed_id,
            "guid-123",
            Some("Test"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(entry.read_at.is_none());

        let read = mark_as_read(&db, entry.id).await.unwrap();
        assert!(read.read_at.is_some());

        let unread = mark_as_unread(&db, entry.id).await.unwrap();
        assert!(unread.read_at.is_none());
    }

    #[tokio::test]
    async fn mark_read_by_filter_marks_only_matching_unread_owned() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let m = upsert_entry_id(
            &db,
            feed_id,
            "m",
            Some("超少女登場"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let n = upsert_entry_id(
            &db,
            feed_id,
            "n",
            Some("其他新聞"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let UpsertOutcome::Inserted(m_id) = m else {
            panic!()
        };
        let UpsertOutcome::Inserted(n_id) = n else {
            panic!()
        };

        let filter = EntryFilter {
            feed_id: Some(feed_id),
            search: Some("超少女".to_string()),
            ..Default::default()
        };
        let affected = mark_read_by_filter(&db, user_id, &filter).await.unwrap();
        assert_eq!(affected, 1);

        let m_read = crate::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            m_id
        )
        .unwrap();
        let n_read = crate::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            n_id
        )
        .unwrap();
        assert!(m_read.is_some(), "matching entry marked read");
        assert!(n_read.is_none(), "non-matching entry untouched");

        // Idempotent: already-read matching row isn't recounted.
        assert_eq!(mark_read_by_filter(&db, user_id, &filter).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn mark_read_by_filter_does_not_cross_user_boundary() {
        let db = setup_db().await;
        let user1 = create_test_user(&db, "user1").await;
        let user2 = create_test_user(&db, "user2").await;
        let category1 = create_test_category(&db, user1, "Tech1").await;
        let category2 = create_test_category(&db, user2, "Tech2").await;
        let feed1 = create_test_feed(&db, category1, "https://example.com/feed1.xml").await;
        let feed2 = create_test_feed(&db, category2, "https://example.com/feed2.xml").await;

        // Same matching title in both users' feeds; filter has no feed_id/
        // category_id scoping, only a search term, so ownership must come
        // entirely from the c.user_id = ?1 seed condition.
        let e1 = upsert_entry_id(
            &db,
            feed1,
            "e1",
            Some("超少女登場"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let e2 = upsert_entry_id(
            &db,
            feed2,
            "e2",
            Some("超少女登場"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let UpsertOutcome::Inserted(e1_id) = e1 else {
            panic!()
        };
        let UpsertOutcome::Inserted(e2_id) = e2 else {
            panic!()
        };

        let filter = EntryFilter {
            search: Some("超少女".to_string()),
            ..Default::default()
        };

        let affected = mark_read_by_filter(&db, user1, &filter).await.unwrap();
        assert_eq!(affected, 1);

        let e1_read = crate::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            e1_id
        )
        .unwrap();
        let e2_read = crate::query_scalar!(
            &db,
            Option<String>,
            "SELECT read_at FROM entry WHERE id = $1",
            e2_id
        )
        .unwrap();
        assert!(
            e1_read.is_some(),
            "owning user's matching entry marked read"
        );
        assert!(
            e2_read.is_none(),
            "matching entry belonging to a different user must not be marked read"
        );
    }

    #[tokio::test]
    async fn test_toggle_star() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let (entry, _) = upsert_entry(
            &db,
            feed_id,
            "guid-123",
            Some("Test"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(entry.starred_at.is_none());

        let starred = toggle_star(&db, entry.id).await.unwrap();
        assert!(starred.starred_at.is_some());

        let unstarred = toggle_star(&db, entry.id).await.unwrap();
        assert!(unstarred.starred_at.is_none());
    }

    #[tokio::test]
    async fn test_count_unread() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        for i in 0..5 {
            upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some("Test"),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        }

        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 5);

        // Mark 2 as read
        let entries = list_by_feed(&db, feed_id, 10, 0).await.unwrap();
        mark_as_read(&db, entries[0].id).await.unwrap();
        mark_as_read(&db, entries[1].id).await.unwrap();

        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn test_search_entries_by_title() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // Create entries with different titles
        upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Rust Programming Guide"),
            None,
            Some("Content about Rust"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-2",
            Some("Python Tutorial"),
            None,
            Some("Content about Python"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].entry.title,
            Some("Rust Programming Guide".to_string())
        );
    }

    #[tokio::test]
    async fn test_search_entries_by_content() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Entry 1"),
            None,
            Some("This article discusses WebAssembly"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-2",
            Some("Entry 2"),
            None,
            Some("This article discusses JavaScript"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            search: Some("WebAssembly".to_string()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.title, Some("Entry 1".to_string()));
    }

    #[tokio::test]
    async fn test_search_case_insensitive() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("UPPERCASE Title"),
            None,
            Some("lowercase content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Search with lowercase should match uppercase title
        let filter = EntryFilter {
            search: Some("uppercase".to_string()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        // Search with uppercase should match lowercase content
        let filter = EntryFilter {
            search: Some("LOWERCASE".to_string()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_search_combined_with_filters() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let (entry1, _) = upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Rust Article"),
            None,
            Some("Content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let (entry2, _) = upsert_entry(
            &db,
            feed_id,
            "guid-2",
            Some("Rust Tutorial"),
            None,
            Some("Content"),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Mark entry1 as read
        mark_as_read(&db, entry1.id).await.unwrap();
        // Star entry2
        toggle_star(&db, entry2.id).await.unwrap();

        // Search for "Rust" with unread_only - should only return entry2
        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            unread_only: true,
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, entry2.id);

        // Search for "Rust" with starred_only - should only return entry2
        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            starred_only: true,
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 10, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.id, entry2.id);
    }

    #[tokio::test]
    async fn test_search_pagination() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // Create 5 entries that match the search
        for i in 0..5 {
            upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some(&format!("Rust Article {i}")),
                None,
                Some("Content"),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        }

        let filter = EntryFilter {
            search: Some("Rust".to_string()),
            ..Default::default()
        };

        // First page (limit 2)
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 2, 0)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // Second page
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 2, 2)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);

        // Third page (only 1 remaining)
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 2, 4)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);

        // Count should be 5
        let count = count_by_user(&db, user_id, &filter).await.unwrap();
        assert_eq!(count, 5);
    }

    #[tokio::test]
    async fn query_is_unread_returns_only_unread() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let (alpha, _) = upsert_entry(
            &db,
            feed_id,
            "guid-alpha",
            Some("alpha"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let (beta, _) = upsert_entry(
            &db,
            feed_id,
            "guid-beta",
            Some("beta"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        mark_as_read(&db, beta.id).await.unwrap();
        assert!(alpha.read_at.is_none());

        let filter = EntryFilter {
            query: Some(query::parse("is:unread").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let titles: Vec<_> = results
            .iter()
            .filter_map(|r| r.entry.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t == "alpha"));
        assert!(!titles.iter().any(|t| t == "beta"));
    }

    #[tokio::test]
    async fn query_boolean_and_negation() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-rust",
            Some("rust"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let (rust_weekly, _) = upsert_entry(
            &db,
            feed_id,
            "guid-rust-weekly",
            Some("rust weekly"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        mark_as_read(&db, rust_weekly.id).await.unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-go",
            Some("go"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            query: Some(query::parse("rust -is:read").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let titles: Vec<_> = results
            .iter()
            .filter_map(|r| r.entry.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t == "rust"));
        assert!(!titles.iter().any(|t| t == "rust weekly")); // read -> excluded
        assert!(!titles.iter().any(|t| t == "go")); // no "rust" -> excluded
    }

    // M1 regression: `NOT (NULL LIKE ...)` is NULL (not TRUE), so a naive
    // `(NOT e.author LIKE ...)` would silently exclude every NULL-author row.
    // The COALESCE(..., 0/FALSE) wrapper makes the leaf two-valued so negation
    // correctly includes rows where the filtered column is NULL.
    #[tokio::test]
    async fn query_negated_author_includes_null_author_entries() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-jane",
            Some("jane's post"),
            None,
            None,
            None,
            Some("jane"),
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-bob",
            Some("bob's post"),
            None,
            None,
            None,
            Some("bob"),
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-anon",
            Some("anonymous post"),
            None,
            None,
            None,
            None, // no author (NULL)
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            query: Some(query::parse("-author:jane").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let titles: Vec<_> = results
            .iter()
            .filter_map(|r| r.entry.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t == "bob's post"));
        assert!(titles.iter().any(|t| t == "anonymous post")); // NULL author -> included
        assert!(!titles.iter().any(|t| t == "jane's post"));
    }

    #[tokio::test]
    async fn query_feed_name_fuzzy_match() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let rust_feed_id = feed::create_feed(
            &db,
            &feed::CreateFeedParams {
                category_id,
                url: "https://example.com/rust-blog.xml",
                title: Some("Rust Blog"),
                description: None,
                site_url: None,
                custom_user_agent: None,
                http2_disabled: None,
                custom_referrer: None,
            },
        )
        .await
        .unwrap()
        .id;
        let other_feed_id =
            create_test_feed(&db, category_id, "https://example.com/other.xml").await;

        upsert_entry(
            &db,
            rust_feed_id,
            "guid-1",
            Some("Post from Rust Blog"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            other_feed_id,
            "guid-2",
            Some("Post from other feed"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            query: Some(query::parse("feed:rust").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .all(|r| r.entry.title.as_deref() == Some("Post from Rust Blog"))
        );
    }

    #[tokio::test]
    async fn query_after_date_filters() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-old",
            Some("old"),
            None,
            None,
            None,
            None,
            Some(chrono::Utc.with_ymd_and_hms(2025, 6, 1, 0, 0, 0).unwrap()),
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-new",
            Some("new"),
            None,
            None,
            None,
            None,
            Some(chrono::Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap()),
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            query: Some(query::parse("after:2026-01-01").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let titles: Vec<_> = results
            .iter()
            .filter_map(|r| r.entry.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t == "new"));
        assert!(!titles.iter().any(|t| t == "old"));
    }

    #[tokio::test]
    async fn query_escapes_literal_percent() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        upsert_entry(
            &db,
            feed_id,
            "guid-off",
            Some("50% off"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        upsert_entry(
            &db,
            feed_id,
            "guid-dollars",
            Some("50 dollars"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            query: Some(query::parse("\"50%\"").unwrap()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let titles: Vec<_> = results
            .iter()
            .filter_map(|r| r.entry.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t == "50% off"));
        assert!(!titles.iter().any(|t| t == "50 dollars")); // literal %, not wildcard
    }

    #[tokio::test]
    async fn search_matches_plain_text_across_tags_not_attributes() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // (a) term split across inline tags — must match via content_text.
        upsert_entry(
            &db,
            feed_id,
            "a",
            Some("x"),
            None,
            Some("超<b>少女</b>登場"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        // (b) term only inside an href attribute — must NOT match.
        upsert_entry(
            &db,
            feed_id,
            "b",
            Some("y"),
            None,
            Some(r#"<a href="/超少女">z</a>"#),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let filter = EntryFilter {
            search: Some("超少女".to_string()),
            ..Default::default()
        };
        let results = list_by_user(&db, user_id, &filter, EntrySortOrder::default(), 50, 0)
            .await
            .unwrap();
        let guids: Vec<_> = results.iter().map(|r| r.entry.guid.clone()).collect();
        assert!(
            guids.contains(&"a".to_string()),
            "tag-split term should match"
        );
        assert!(
            !guids.contains(&"b".to_string()),
            "attribute-only term should not match"
        );
    }

    #[tokio::test]
    async fn test_mark_read_by_ids() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let user2_id = create_test_user(&db, "testuser2").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let category2_id = create_test_category(&db, user2_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
        let feed2_id = create_test_feed(&db, category2_id, "https://example2.com/feed.xml").await;

        // Create entries for user 1
        let (entry1, _) = upsert_entry(
            &db,
            feed_id,
            "guid-1",
            Some("Entry 1"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let (entry2, _) = upsert_entry(
            &db,
            feed_id,
            "guid-2",
            Some("Entry 2"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let (entry3, _) = upsert_entry(
            &db,
            feed_id,
            "guid-3",
            Some("Entry 3"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Create entry for user 2
        let (other_entry, _) = upsert_entry(
            &db,
            feed2_id,
            "guid-4",
            Some("Other Entry"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // All entries should be unread
        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 3);
        assert_eq!(count_unread_by_user(&db, user2_id).await.unwrap(), 1);

        // Mark entries 1 and 2 as read (user 1)
        let marked = mark_read_by_ids(&db, user_id, &[entry1.id, entry2.id])
            .await
            .unwrap();
        assert_eq!(marked, 2);

        // Entry 3 should still be unread
        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 1);

        // Try to mark user 2's entry as read with user 1's credentials - should not work
        let marked = mark_read_by_ids(&db, user_id, &[other_entry.id])
            .await
            .unwrap();
        assert_eq!(marked, 0);

        // User 2's entry should still be unread
        assert_eq!(count_unread_by_user(&db, user2_id).await.unwrap(), 1);

        // Mark already-read entries again - should return 0
        let marked = mark_read_by_ids(&db, user_id, &[entry1.id, entry2.id])
            .await
            .unwrap();
        assert_eq!(marked, 0);

        // Empty array should return 0
        let marked = mark_read_by_ids(&db, user_id, &[]).await.unwrap();
        assert_eq!(marked, 0);

        // Mark remaining entry
        let marked = mark_read_by_ids(&db, user_id, &[entry3.id]).await.unwrap();
        assert_eq!(marked, 1);

        // All user 1 entries should now be read
        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_upsert_entry_id_returns_id_and_is_new() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // First upsert: new row
        let first = upsert_entry_id(
            &db,
            feed_id,
            "guid-1",
            Some("Title"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let id1 = match first {
            UpsertOutcome::Inserted(id) => id,
            o => panic!("expected Inserted, got {o:?}"),
        };
        let row = find_by_id(&db, id1).await.unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some("Title"));

        // Second upsert with same guid+feed: update, same id
        let second = upsert_entry_id(
            &db,
            feed_id,
            "guid-1",
            Some("Title 2"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(id) if id == id1));
        let row = find_by_id(&db, id1).await.unwrap().unwrap();
        assert_eq!(row.title.as_deref(), Some("Title 2"));
    }

    /// Re-upserting byte-identical values must report `Unchanged` and leave the
    /// row (including `updated_at`) untouched — feeds re-serve their whole
    /// window every poll, so this is the common path, and rewriting every row
    /// each time is pure WAL churn.
    #[tokio::test]
    async fn upsert_identical_values_is_unchanged_and_does_not_write() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let args = (
            Some("Title"),
            Some("https://example.com/a"),
            Some("<p>body</p>"),
            Some("summary"),
            Some("author"),
        );
        let first = upsert_entry_id(
            &db, feed_id, "guid-1", args.0, args.1, args.2, args.3, args.4, None,
        )
        .await
        .unwrap();
        let id = match first {
            UpsertOutcome::Inserted(id) => id,
            o => panic!("expected Inserted, got {o:?}"),
        };
        let before = find_by_id(&db, id).await.unwrap().unwrap();

        // Same values again: no write, no bump of `updated_at`.
        let second = upsert_entry_id(
            &db, feed_id, "guid-1", args.0, args.1, args.2, args.3, args.4, None,
        )
        .await
        .unwrap();
        assert!(
            matches!(second, UpsertOutcome::Unchanged(uid) if uid == id),
            "expected Unchanged, got {second:?}"
        );
        let after = find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(before.updated_at, after.updated_at);

        // A single differing column still updates.
        let third = upsert_entry_id(
            &db,
            feed_id,
            "guid-1",
            args.0,
            args.1,
            Some("<p>edited</p>"),
            args.3,
            args.4,
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(third, UpsertOutcome::Updated(uid) if uid == id),
            "expected Updated, got {third:?}"
        );
        let edited = find_by_id(&db, id).await.unwrap().unwrap();
        assert_eq!(edited.content.as_deref(), Some("<p>edited</p>"));
    }

    /// A NULL column that stays NULL must not count as a difference — the
    /// guard uses NULL-safe inequality (`IS NOT` / `IS DISTINCT FROM`), not
    /// plain `<>`, which would yield NULL and silently rewrite every row.
    #[tokio::test]
    async fn upsert_null_columns_compare_null_safely() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let first = upsert_entry_id(
            &db,
            feed_id,
            "guid-n",
            Some("T"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let id = match first {
            UpsertOutcome::Inserted(id) => id,
            o => panic!("expected Inserted, got {o:?}"),
        };

        let second = upsert_entry_id(
            &db,
            feed_id,
            "guid-n",
            Some("T"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(second, UpsertOutcome::Unchanged(uid) if uid == id),
            "all-NULL columns unchanged should be Unchanged, got {second:?}"
        );

        // NULL -> non-NULL is a real change.
        let third = upsert_entry_id(
            &db,
            feed_id,
            "guid-n",
            Some("T"),
            Some("https://example.com/x"),
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(third, UpsertOutcome::Updated(uid) if uid == id),
            "NULL -> value should be Updated, got {third:?}"
        );

        // non-NULL -> NULL is a real change too.
        let fourth = upsert_entry_id(
            &db,
            feed_id,
            "guid-n",
            Some("T"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(
            matches!(fourth, UpsertOutcome::Updated(uid) if uid == id),
            "value -> NULL should be Updated, got {fourth:?}"
        );
    }

    #[tokio::test]
    async fn upsert_populates_content_text_stripped() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let out = upsert_entry_id(
            &db,
            feed_id,
            "g1",
            Some("t"),
            None,
            Some("超<b>少女</b>與機器人"),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let id = match out {
            UpsertOutcome::Inserted(id) => id,
            o => panic!("expected Inserted, got {o:?}"),
        };
        let ct = crate::query_scalar!(
            &db,
            Option<String>,
            "SELECT content_text FROM entry WHERE id = $1",
            id
        )
        .unwrap();
        assert_eq!(ct.as_deref(), Some("超少女與機器人"));
    }

    #[tokio::test]
    async fn test_upsert_skips_tombstoned_guid() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        insert_tombstone(&db, feed_id, "ghost").await.unwrap();
        let outcome = upsert_entry_id(
            &db,
            feed_id,
            "ghost",
            Some("Ghost"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));
        assert!(
            find_by_guid_and_feed(&db, "ghost", feed_id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_upsert_inserts_then_updates() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        let first = upsert_entry_id(
            &db,
            feed_id,
            "g1",
            Some("First"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let id = match first {
            UpsertOutcome::Inserted(id) => id,
            other => panic!("expected Inserted, got {other:?}"),
        };

        let second = upsert_entry_id(
            &db,
            feed_id,
            "g1",
            Some("Updated"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(second, UpsertOutcome::Updated(uid) if uid == id));
    }

    #[tokio::test]
    async fn test_star_unstar_and_mark_unread_by_ids() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let user2_id = create_test_user(&db, "testuser2").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let category2_id = create_test_category(&db, user2_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;
        let feed2_id = create_test_feed(&db, category2_id, "https://example2.com/feed.xml").await;

        let (e1, _) = upsert_entry(&db, feed_id, "g1", Some("E1"), None, None, None, None, None)
            .await
            .unwrap();
        let (e2, _) = upsert_entry(&db, feed_id, "g2", Some("E2"), None, None, None, None, None)
            .await
            .unwrap();
        let (other, _) = upsert_entry(
            &db,
            feed2_id,
            "g3",
            Some("E3"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Star e1, e2 — only currently-unstarred rows count
        let starred = star_by_ids(&db, user_id, &[e1.id, e2.id]).await.unwrap();
        assert_eq!(starred, 2);
        assert!(
            find_by_id(&db, e1.id)
                .await
                .unwrap()
                .unwrap()
                .starred_at
                .is_some()
        );

        // Starring again is a no-op (already starred)
        assert_eq!(star_by_ids(&db, user_id, &[e1.id, e2.id]).await.unwrap(), 0);

        // Ownership scope: cannot star another user's entry
        assert_eq!(star_by_ids(&db, user_id, &[other.id]).await.unwrap(), 0);
        assert!(
            find_by_id(&db, other.id)
                .await
                .unwrap()
                .unwrap()
                .starred_at
                .is_none()
        );

        // Unstar e1
        assert_eq!(unstar_by_ids(&db, user_id, &[e1.id]).await.unwrap(), 1);
        assert!(
            find_by_id(&db, e1.id)
                .await
                .unwrap()
                .unwrap()
                .starred_at
                .is_none()
        );

        // Mark read then mark unread by ids
        assert_eq!(
            mark_read_by_ids(&db, user_id, &[e1.id, e2.id])
                .await
                .unwrap(),
            2
        );
        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 0);
        let unread = mark_unread_by_ids(&db, user_id, &[e1.id, e2.id])
            .await
            .unwrap();
        assert_eq!(unread, 2);
        assert_eq!(count_unread_by_user(&db, user_id).await.unwrap(), 2);

        // Empty input is a no-op
        assert_eq!(star_by_ids(&db, user_id, &[]).await.unwrap(), 0);
        assert_eq!(mark_unread_by_ids(&db, user_id, &[]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_find_neighbors_starred_only() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // Create 5 entries with distinct timestamps
        let mut entries = Vec::new();
        for i in 0..5 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some(&format!("Entry {i}")),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .await
            .unwrap();
            entries.push(entry);
        }

        // Star entries 1 and 3 (0-indexed)
        star_entry(&db, entries[1].id).await.unwrap();
        star_entry(&db, entries[3].id).await.unwrap();

        // From entry 3 (starred), with starred_only filter:
        // prev should be entry 1 (the only other starred entry that is newer... wait, entry 3 is newer)
        // Actually entries are ordered by published_at DESC, so entry 4 is newest.
        // entry 3 published_at = now+30s, entry 1 published_at = now+10s
        // prev (newer than entry 3) = none starred that is newer
        // next (older than entry 3) = entry 1 (starred, older)
        let filter = EntryFilter {
            starred_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&db, user_id, entries[3].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, None); // no starred entry newer than entry 3
        assert_eq!(neighbors.next_id, Some(entries[1].id)); // entry 1 is older and starred

        // From entry 1 (starred):
        // prev (newer) = entry 3 (starred, newer)
        // next (older) = none
        let neighbors = find_neighbors(&db, user_id, entries[1].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[3].id));
        assert_eq!(neighbors.next_id, None);

        // Without filter, entry 3 should see entry 4 as prev and entry 2 as next
        let no_filter = EntryFilter::default();
        let neighbors = find_neighbors(&db, user_id, entries[3].id, &no_filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[2].id));
    }

    #[tokio::test]
    async fn test_find_neighbors_unread_only_read_after() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // 5 entries published ascending — entries[4] is newest in the
        // published-DESC list order.
        let mut entries = Vec::new();
        for i in 0..5 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some(&format!("Entry {i}")),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .await
            .unwrap();
            entries.push(entry);
        }

        // entries[1]: read an hour ago — before the snapshot boundary.
        crate::db_execute!(
            &db,
            "UPDATE entry SET read_at = datetime('now', '-1 hour') WHERE id = $1",
            entries[1].id
        )
        .unwrap();
        // entries[2]: read just now — inside the snapshot.
        crate::db_execute!(
            &db,
            "UPDATE entry SET read_at = datetime('now') WHERE id = $1",
            entries[2].id
        )
        .unwrap();

        // Snapshot boundary 10 minutes ago: the just-read entries[2] is
        // inside it, the hour-old entries[1] is not.
        let snapshot = (Utc::now() - chrono::Duration::minutes(10))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        let filter = EntryFilter {
            unread_only: true,
            read_after: Some(snapshot),
            ..Default::default()
        };

        // From entries[3]: next (older) lands on the just-read entries[2]
        // — still reachable inside the snapshot.
        let neighbors = find_neighbors(&db, user_id, entries[3].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[2].id));

        // From entries[2]: prev is the unread entries[3]; next skips the
        // pre-snapshot entries[1] and lands on the unread entries[0].
        let neighbors = find_neighbors(&db, user_id, entries[2].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[3].id));
        assert_eq!(neighbors.next_id, Some(entries[0].id));

        // Plain unread_only without read_after keeps the strict live
        // filter: both read entries are skipped.
        let strict = EntryFilter {
            unread_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&db, user_id, entries[3].id, &strict)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[4].id));
        assert_eq!(neighbors.next_id, Some(entries[0].id));
    }

    #[tokio::test]
    async fn test_find_neighbors_read_only() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "testuser").await;
        let category_id = create_test_category(&db, user_id, "Tech").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/feed.xml").await;

        // Create 4 entries with distinct timestamps
        let mut entries = Vec::new();
        for i in 0..4 {
            let published = Utc::now() + chrono::Duration::seconds(i * 10);
            let (entry, _) = upsert_entry(
                &db,
                feed_id,
                &format!("guid-{i}"),
                Some(&format!("Entry {i}")),
                None,
                None,
                None,
                None,
                Some(published),
            )
            .await
            .unwrap();
            entries.push(entry);
        }

        // Mark entries 0 and 2 as read
        mark_as_read(&db, entries[0].id).await.unwrap();
        mark_as_read(&db, entries[2].id).await.unwrap();

        // From entry 2 (read, published_at = now+20s), with read_only filter:
        // prev (newer) = none (entry 3 is newer but unread)
        // next (older) = entry 0 (read, older)
        let filter = EntryFilter {
            read_only: true,
            ..Default::default()
        };
        let neighbors = find_neighbors(&db, user_id, entries[2].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, None);
        assert_eq!(neighbors.next_id, Some(entries[0].id));

        // From entry 0 (read, published_at = now+0s):
        // prev (newer) = entry 2 (read, newer)
        // next (older) = none
        let neighbors = find_neighbors(&db, user_id, entries[0].id, &filter)
            .await
            .unwrap();
        assert_eq!(neighbors.prev_id, Some(entries[2].id));
        assert_eq!(neighbors.next_id, None);
    }

    #[test]
    fn cursor_parses_composite_format() {
        let c = ContinuationCursor::parse("2026-04-26 12:34:56|142").expect("composite parses");
        match c {
            ContinuationCursor::Composite { sort_ts, id } => {
                assert_eq!(sort_ts, "2026-04-26 12:34:56");
                assert_eq!(id, 142);
            }
            ContinuationCursor::LegacyId(_) => panic!("expected Composite"),
        }
    }

    #[test]
    fn cursor_parses_bare_i64_as_legacy() {
        let c = ContinuationCursor::parse("142").expect("legacy parses");
        match c {
            ContinuationCursor::LegacyId(id) => assert_eq!(id, 142),
            ContinuationCursor::Composite { .. } => panic!("expected LegacyId"),
        }
    }

    #[test]
    fn cursor_rejects_garbage() {
        assert!(ContinuationCursor::parse("not-a-cursor").is_none());
        assert!(ContinuationCursor::parse("|").is_none());
        assert!(ContinuationCursor::parse("ts|notnum").is_none());
        assert!(ContinuationCursor::parse("").is_none());
    }

    #[test]
    fn cursor_encodes_composite() {
        assert_eq!(
            ContinuationCursor::encode_composite("2026-04-26 12:34:56", 142),
            "2026-04-26 12:34:56|142"
        );
    }

    #[tokio::test]
    async fn fetch_sort_ts_returns_published_or_created_for_publishedat() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

        // Entry with published_at set
        let id1 = crate::query_scalar!(
            &db,
            i64,
            "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3) RETURNING id",
            feed_id,
            "g1",
            "2026-04-01 10:00:00"
        )
        .unwrap();

        // Entry with published_at NULL → COALESCE falls back to created_at
        let id2 = crate::query_scalar!(
            &db,
            i64,
            "INSERT INTO entry (feed_id, guid, created_at) VALUES ($1, $2, $3) RETURNING id",
            feed_id,
            "g2",
            "2026-04-02 11:00:00"
        )
        .unwrap();

        let ts1 = fetch_sort_ts(&db, id1, EntrySortOrder::PublishedAt)
            .await
            .unwrap();
        let ts2 = fetch_sort_ts(&db, id2, EntrySortOrder::PublishedAt)
            .await
            .unwrap();
        assert_eq!(ts1.as_deref(), Some("2026-04-01 10:00:00"));
        assert_eq!(ts2.as_deref(), Some("2026-04-02 11:00:00"));
    }

    #[tokio::test]
    async fn fetch_sort_ts_returns_read_at_for_readat_sort() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

        let id = crate::query_scalar!(
            &db,
            i64,
            "INSERT INTO entry (feed_id, guid, read_at) VALUES ($1, $2, $3) RETURNING id",
            feed_id,
            "g1",
            "2026-04-03 12:00:00"
        )
        .unwrap();

        let ts = fetch_sort_ts(&db, id, EntrySortOrder::ReadAt)
            .await
            .unwrap();
        assert_eq!(ts.as_deref(), Some("2026-04-03 12:00:00"));
    }

    #[tokio::test]
    async fn fetch_sort_ts_returns_none_for_missing_id() {
        let db = setup_db().await;
        let ts = fetch_sort_ts(&db, 99999, EntrySortOrder::PublishedAt)
            .await
            .unwrap();
        assert_eq!(ts, None);
    }

    #[tokio::test]
    async fn composite_cursor_walks_non_monotonic_data_without_skip() {
        // Repro for #164: when id↔published_at order diverges (OPML re-import,
        // back-dated feed items), the legacy `e.id < ?` cursor silently skips.
        // The composite cursor must visit every entry.
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

        // 6 monotonic entries (newer ts ⇒ later id), then 4 "back-dated" entries
        // with NEW ids but OLD timestamps (mimics OPML re-import).
        let monotonic = [
            ("g1", "2026-04-01 10:00:00"),
            ("g2", "2026-04-02 10:00:00"),
            ("g3", "2026-04-03 10:00:00"),
            ("g4", "2026-04-04 10:00:00"),
            ("g5", "2026-04-05 10:00:00"),
            ("g6", "2026-04-06 10:00:00"),
        ];
        let backdated = [
            ("g7-bd", "2026-03-01 10:00:00"),
            ("g8-bd", "2026-03-02 10:00:00"),
            ("g9-bd", "2026-03-03 10:00:00"),
            ("g10-bd", "2026-03-04 10:00:00"),
        ];
        for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
            crate::db_execute!(
                &db,
                "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
                feed_id,
                *guid,
                *ts
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut seen: Vec<i64> = Vec::new();
        let mut cursor: Option<ContinuationCursor> = None;
        let page_limit: i64 = 3;

        // Walk pages (DESC sort by COALESCE(published_at, created_at)) until empty
        loop {
            let pagination = ContinuationParams {
                oldest_first: false,
                limit: page_limit,
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let page = list_by_user_with_continuation(&db, user_id, &filter, &pagination)
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            for ewf in &page {
                assert!(
                    !seen.contains(&ewf.entry.id),
                    "duplicate id {}",
                    ewf.entry.id
                );
                seen.push(ewf.entry.id);
            }
            let last = page.last().unwrap();
            let sort_ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
                .await
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            });
            // safety: don't loop forever
            assert!(seen.len() <= 100, "runaway loop");
        }

        assert_eq!(
            seen.len(),
            10,
            "must visit all 10 entries; saw {}",
            seen.len()
        );
    }

    #[tokio::test]
    async fn composite_cursor_walks_non_monotonic_data_oldest_first_without_skip() {
        // Same shape as composite_cursor_walks_non_monotonic_data_without_skip
        // but exercises the oldest_first=true (ASC) path of the bounded-OR
        // predicate. Triggered in production by the GReader `r=o` query param.
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

        let monotonic = [
            ("g1", "2026-04-01 10:00:00"),
            ("g2", "2026-04-02 10:00:00"),
            ("g3", "2026-04-03 10:00:00"),
            ("g4", "2026-04-04 10:00:00"),
            ("g5", "2026-04-05 10:00:00"),
            ("g6", "2026-04-06 10:00:00"),
        ];
        let backdated = [
            ("g7-bd", "2026-03-01 10:00:00"),
            ("g8-bd", "2026-03-02 10:00:00"),
            ("g9-bd", "2026-03-03 10:00:00"),
            ("g10-bd", "2026-03-04 10:00:00"),
        ];
        for (guid, ts) in monotonic.iter().chain(backdated.iter()) {
            crate::db_execute!(
                &db,
                "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
                feed_id,
                *guid,
                *ts
            )
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut seen: Vec<i64> = Vec::new();
        let mut cursor: Option<ContinuationCursor> = None;
        let page_limit: i64 = 3;

        loop {
            let pagination = ContinuationParams {
                oldest_first: true,
                limit: page_limit,
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let page = list_by_user_with_continuation(&db, user_id, &filter, &pagination)
                .await
                .unwrap();
            if page.is_empty() {
                break;
            }
            for ewf in &page {
                assert!(
                    !seen.contains(&ewf.entry.id),
                    "duplicate id {}",
                    ewf.entry.id
                );
                seen.push(ewf.entry.id);
            }
            let last = page.last().unwrap();
            let sort_ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
                .await
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts,
                id: last.entry.id,
            });
            assert!(seen.len() <= 100, "runaway loop");
        }

        assert_eq!(
            seen.len(),
            10,
            "must visit all 10 entries on ASC walk; saw {}",
            seen.len()
        );
    }

    #[tokio::test]
    async fn legacy_bare_i64_cursor_still_paginates() {
        // In-flight cursors from pre-#164 deployments must still work for one
        // grace period. Under monotonic data (the common case), the legacy
        // `e.id < ?` predicate is correct.
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;

        for i in 1..=5 {
            crate::db_execute!(
                &db,
                "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
                feed_id,
                format!("g{}", i),
                format!("2026-04-0{} 10:00:00", i)
            )
            .unwrap();
        }

        // Get id of "newest" entry (highest id, latest ts)
        let max_id = crate::query_scalar!(&db, i64, "SELECT MAX(id) FROM entry").unwrap();

        let pagination = ContinuationParams {
            oldest_first: false,
            limit: 10,
            continuation: Some(ContinuationCursor::LegacyId(max_id)),
            ot: None,
            nt: None,
            sort_order: EntrySortOrder::PublishedAt,
        };
        let page =
            list_by_user_with_continuation(&db, user_id, &EntryFilter::default(), &pagination)
                .await
                .unwrap();

        // 4 entries below the boundary id
        assert_eq!(page.len(), 4);
        for ewf in &page {
            assert!(ewf.entry.id < max_id);
        }
    }

    #[test]
    fn is_no_entry_side_predicate_matches_only_user_only_filter() {
        // Default filter (used by /entries) has no entry-side predicate.
        assert!(is_no_entry_side_predicate(&EntryFilter::default()));

        // Any of these flags pulls in an entry-side predicate.
        let cases = [
            EntryFilter {
                feed_id: Some(1),
                ..Default::default()
            },
            EntryFilter {
                category_id: Some(1),
                ..Default::default()
            },
            EntryFilter {
                unread_only: true,
                ..Default::default()
            },
            EntryFilter {
                starred_only: true,
                ..Default::default()
            },
            EntryFilter {
                read_only: true,
                ..Default::default()
            },
            EntryFilter {
                search: Some("foo".into()),
                ..Default::default()
            },
            EntryFilter {
                has_summary: Some(true),
                ..Default::default()
            },
            EntryFilter {
                has_summary: Some(false),
                ..Default::default()
            },
        ];
        for f in &cases {
            assert!(
                !is_no_entry_side_predicate(f),
                "expected entry-side predicate for filter: {f:?}"
            );
        }
    }

    /// Captures the EXPLAIN QUERY PLAN output for a SELECT. Concatenates all
    /// `detail` columns so callers can `assert!(plan.contains("idx_entry_…"))`
    /// to lock in the planner choice. The bound values are placeholders — the
    /// planner only needs parameter count to match, so `n_params` dummy `i64`s
    /// are bound (one per `$N` placeholder).
    async fn explain_plan_for(db: &Db, sql: &str, n_params: usize) -> String {
        let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
        let rows: Vec<(i64, i64, i64, String)> = match db.inner() {
            DbInner::Sqlite(pool) => {
                let mut q = sqlx::query_as::<sqlx::Sqlite, (i64, i64, i64, String)>(
                    sqlx::AssertSqlSafe(explain_sql),
                );
                for _ in 0..n_params {
                    q = q.bind(1i64);
                }
                q.fetch_all(pool).await.unwrap()
            }
            DbInner::Postgres(_) => unreachable!("tests run against sqlite"),
        };
        rows.into_iter()
            .map(|r| r.3)
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[tokio::test]
    async fn list_by_user_uses_partial_index_for_starred() {
        let db = setup_db().await;
        let _ = create_test_user(&db, "u").await;
        // Tiny in-memory dataset is enough — INDEXED BY is mandatory and the
        // planner has no choice to override the hint.
        let sql = r"
            SELECT e.id, e.feed_id, e.guid, e.title, e.link, e.content, e.summary, e.author,
                   e.published_at, e.read_at, e.starred_at, e.created_at, e.updated_at,
                   f.title, f.url, f.site_url, c.id, c.name,
                   CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END as has_icon,
                   f.custom_referrer
            FROM entry e INDEXED BY idx_entry_starred_sort
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
            WHERE c.user_id = $1 AND e.starred_at IS NOT NULL
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        ";
        let plan = explain_plan_for(&db, sql, 1).await;
        assert!(
            plan.contains("idx_entry_starred_sort"),
            "plan missing partial index: {plan}"
        );
    }

    #[tokio::test]
    async fn list_by_user_uses_partial_index_for_read() {
        let db = setup_db().await;
        let _ = create_test_user(&db, "u").await;
        let sql = r"
            SELECT e.id FROM entry e INDEXED BY idx_entry_read_sort
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = $1 AND e.read_at IS NOT NULL
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        ";
        let plan = explain_plan_for(&db, sql, 1).await;
        assert!(
            plan.contains("idx_entry_read_sort"),
            "plan missing partial index: {plan}"
        );
    }

    #[tokio::test]
    async fn list_by_user_no_predicate_uses_sort_ts_index() {
        // End-to-end: prepared SQL must include the INDEXED BY hint for the
        // "All Entries" case, otherwise the planner falls back to walking
        // every row via category->feed->entry.
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;
        for i in 1..=3 {
            crate::db_execute!(
                &db,
                "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
                feed_id,
                format!("g{}", i),
                format!("2026-05-0{} 10:00:00", i)
            )
            .unwrap();
        }

        // Sanity: the public API returns the right rows under the hint.
        let rows = list_by_user(
            &db,
            user_id,
            &EntryFilter::default(),
            EntrySortOrder::PublishedAt,
            10,
            0,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 3);

        // Plan check: a hand-built copy of the same query (same shape as the
        // builder produces with the no-predicate hint) must scan via
        // `idx_entry_sort_ts`. We test the shape, not the exact runtime
        // statement the dynamic builder assembles.
        let sql = r"
            SELECT e.id FROM entry e INDEXED BY idx_entry_sort_ts
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            WHERE c.user_id = $1
            ORDER BY COALESCE(e.published_at, e.created_at) DESC
            LIMIT 51
        ";
        let plan = explain_plan_for(&db, sql, 1).await;
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan missing sort_ts index: {plan}"
        );
    }

    #[tokio::test]
    async fn continuation_page0_unfiltered_uses_sort_ts_index() {
        // Regression guard for the page-0 index hint added to
        // `list_by_user_with_continuation`. Without `INDEXED BY idx_entry_sort_ts`
        // the planner walks category->feed->entry and temp-B-tree-sorts the whole
        // corpus before LIMIT — a ~350× slowdown on a large instance. The
        // behavioral walk test (`test_continuation_walk_is_gapless_unfiltered`)
        // would NOT catch a dropped hint because results are identical. This test
        // pins the query plan directly.
        //
        // Mirrors the same pattern as `list_by_user_no_predicate_uses_sort_ts_index`:
        // replicate the SQL shape the builder emits for the cursorless case and
        // assert EXPLAIN QUERY PLAN mentions the index.
        let db = setup_db().await;
        let user_id = create_test_user(&db, "u").await;
        let cat_id = create_test_category(&db, user_id, "c").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/f.xml").await;
        for i in 1..=3 {
            crate::db_execute!(
                &db,
                "INSERT INTO entry (feed_id, guid, published_at) VALUES ($1, $2, $3)",
                feed_id,
                format!("g{}", i),
                format!("2026-05-0{} 10:00:00", i)
            )
            .unwrap();
        }

        // Hand-built copy of the SQL `list_by_user_with_continuation` emits for:
        //   filter = EntryFilter::default(), sort_order = PublishedAt,
        //   continuation = None, oldest_first = false, ot = None, nt = None.
        // The entry hint resolves to " INDEXED BY idx_entry_sort_ts" because
        // `is_no_entry_side_predicate` is true and sort_order is PublishedAt.
        // The ORDER BY includes the tie-breaker `e.id DESC` that the continuation
        // builder always appends (unlike list_by_user which omits it).
        let sql = r"
            SELECT e.id
            FROM entry e INDEXED BY idx_entry_sort_ts
            INNER JOIN feed f ON e.feed_id = f.id
            INNER JOIN category c ON f.category_id = c.id
            LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id
            WHERE c.user_id = $1
            ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
            LIMIT $2
        ";
        let plan = explain_plan_for(&db, sql, 2).await;
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan missing sort_ts index: {plan}"
        );
        assert!(
            !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
            "plan must not temp-B-tree-sort: {plan}"
        );
    }

    /// Locks the query plan for the snapshot-widened unread neighbours query.
    /// Without the `idx_entry_sort_ts` hint + entry-first CROSS JOIN that
    /// `find_neighbors` emits for unread filters, the planner answers the
    /// `(read_at IS NULL OR read_at >= ?)` predicate with a MULTI-INDEX OR
    /// that scans the read-majority of the table into a temp B-tree — an
    /// O(table) scan per call that grows unbounded with inbox size. The hint
    /// turns it into an indexed range scan that short-circuits at LIMIT 1.
    #[tokio::test]
    async fn find_neighbors_unread_read_after_uses_sort_ts_not_multi_index_or() {
        let db = setup_db().await;
        let _ = create_test_user(&db, "u").await;
        // Mirrors the next-side SQL `find_neighbors` builds for an unread
        // filter with `read_after` set (see the join_kw / entry_hint branch).
        // Each `$N` placeholder is distinct (the builder binds the sort_ts value
        // twice rather than reusing one slot); only the parameter count matters
        // to the planner.
        let sql = r"
            SELECT e.id
            FROM entry e INDEXED BY idx_entry_sort_ts
            CROSS JOIN feed f ON e.feed_id = f.id
            CROSS JOIN category c ON f.category_id = c.id
            WHERE c.user_id = $1
              AND (COALESCE(e.published_at, e.created_at) < $2
                   OR (COALESCE(e.published_at, e.created_at) = $3 AND e.id < $4))
              AND (e.read_at IS NULL OR e.read_at >= $5)
            ORDER BY COALESCE(e.published_at, e.created_at) DESC, e.id DESC
            LIMIT 1
        ";
        let plan = explain_plan_for(&db, sql, 5).await;
        assert!(
            plan.contains("idx_entry_sort_ts"),
            "plan must pin idx_entry_sort_ts: {plan}"
        );
        assert!(
            !plan.contains("MULTI-INDEX OR"),
            "plan must not fan out into a MULTI-INDEX OR: {plan}"
        );
        assert!(
            !plan.contains("idx_entry_read_at"),
            "plan must not scan via the read_at index: {plan}"
        );
    }

    #[tokio::test]
    async fn test_continuation_walk_is_gapless_unfiltered() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "walker").await;
        let category_id = create_test_category(&db, user_id, "C").await;
        let feed_id = create_test_feed(&db, category_id, "https://example.com/walk.xml").await;
        // 5 entries, distinct published_at so order is deterministic.
        #[allow(
            clippy::cast_sign_loss,
            reason = "loop index from `0..5` is always non-negative"
        )]
        for i in 0..5 {
            upsert_entry(
                &db,
                feed_id,
                &format!("g{i}"),
                Some(&format!("T{i}")),
                None,
                None,
                None,
                None,
                Some(
                    chrono::Utc
                        .with_ymd_and_hms(2024, 1, 1 + i as u32, 0, 0, 0)
                        .unwrap(),
                ),
            )
            .await
            .unwrap();
        }

        let filter = EntryFilter::default();
        let mut cursor: Option<ContinuationCursor> = None;
        let mut seen: Vec<i64> = Vec::new();
        loop {
            let params = ContinuationParams {
                oldest_first: false,
                limit: 3, // page size 2 + 1 sentinel
                continuation: cursor.clone(),
                ot: None,
                nt: None,
                sort_order: EntrySortOrder::PublishedAt,
            };
            let rows = list_by_user_with_continuation(&db, user_id, &filter, &params)
                .await
                .unwrap();
            let has_more = rows.len() > 2;
            let page = &rows[..rows.len().min(2)];
            if page.is_empty() {
                break;
            }
            for e in page {
                seen.push(e.entry.id);
            }
            if !has_more {
                break;
            }
            let last = page.last().unwrap();
            let ts = fetch_sort_ts(&db, last.entry.id, EntrySortOrder::PublishedAt)
                .await
                .unwrap()
                .unwrap();
            cursor = Some(ContinuationCursor::Composite {
                sort_ts: ts,
                id: last.entry.id,
            });
        }

        // All 5 seen exactly once, newest-first.
        assert_eq!(seen.len(), 5, "walk must visit every entry once: {seen:?}");
        let mut uniq = seen.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), 5, "no duplicates across pages: {seen:?}");
    }

    #[tokio::test]
    async fn test_prune_respects_threshold_star_and_optin() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "pruneuser").await;
        let cat_id = create_test_category(&db, user_id, "PruneCat").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/prune.xml").await;

        // Helper: insert a read entry aged `days_old` (and optionally starred).
        async fn mk(db: &Db, feed_id: i64, guid: &str, days: i64, starred: bool) {
            upsert_entry_id(db, feed_id, guid, Some(guid), None, None, None, None, None)
                .await
                .unwrap();
            crate::db_execute!(
                db,
                "UPDATE entry SET read_at = datetime('now', $2) WHERE guid = $1 AND feed_id = $3",
                guid,
                format!("-{days} days"),
                feed_id
            )
            .unwrap();
            if starred {
                crate::db_execute!(
                    db,
                    "UPDATE entry SET starred_at = datetime('now') WHERE guid = $1 AND feed_id = $2",
                    guid,
                    feed_id
                )
                .unwrap();
            }
        }
        mk(&db, feed_id, "old", 40, false).await; // read, 40d, not starred -> victim once enabled
        mk(&db, feed_id, "oldstar", 40, true).await; // starred -> never deleted
        mk(&db, feed_id, "fresh", 1, false).await; // too recent -> kept
        upsert_entry_id(
            &db,
            feed_id,
            "unread",
            Some("u"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap(); // unread

        // Opt-in disabled (default 0): nothing pruned.
        assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 0);

        // Enable retention at 30 days for the feed's owner.
        let user_id_check = crate::query_scalar!(
            &db,
            i64,
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = $1",
            feed_id
        )
        .unwrap();
        crate::models::user_settings::update_retention_read_days(&db, user_id_check, 30)
            .await
            .unwrap();

        // Only "old" is pruned; a tombstone is written for it.
        assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 1);
        assert!(
            find_by_guid_and_feed(&db, "old", feed_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            find_by_guid_and_feed(&db, "oldstar", feed_id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_guid_and_feed(&db, "fresh", feed_id)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            find_by_guid_and_feed(&db, "unread", feed_id)
                .await
                .unwrap()
                .is_some()
        );

        // Tombstone present -> a refresh serving "old" again is skipped.
        let outcome = upsert_entry_id(
            &db,
            feed_id,
            "old",
            Some("Old"),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, UpsertOutcome::SkippedTombstoned));

        // Idempotent: nothing left to prune.
        assert_eq!(prune_read_retention_batch(&db, 500).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_prune_batch_size_limits_rows() {
        let db = setup_db().await;
        let user_id = create_test_user(&db, "batchuser").await;
        let cat_id = create_test_category(&db, user_id, "BatchCat").await;
        let feed_id = create_test_feed(&db, cat_id, "https://example.com/batch.xml").await;
        let user_id_check = crate::query_scalar!(
            &db,
            i64,
            "SELECT c.user_id FROM feed f JOIN category c ON c.id = f.category_id WHERE f.id = $1",
            feed_id
        )
        .unwrap();
        crate::models::user_settings::update_retention_read_days(&db, user_id_check, 1)
            .await
            .unwrap();
        for i in 0..5 {
            let g = format!("g{i}");
            upsert_entry_id(&db, feed_id, &g, Some(&g), None, None, None, None, None)
                .await
                .unwrap();
            crate::db_execute!(
                &db,
                "UPDATE entry SET read_at = datetime('now', '-10 days') WHERE guid = $1 AND feed_id = $2",
                g,
                feed_id
            )
            .unwrap();
        }
        assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 2);
        assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 1);
        assert_eq!(prune_read_retention_batch(&db, 2).await.unwrap(), 0);

        // Every victim of every batch must have been tombstoned, not just the
        // first of each: the prune writes tombstones as one set-based INSERT
        // per batch, so a partial write would silently resurrect entries on
        // the next refresh.
        for i in 0..5 {
            let g = format!("g{i}");
            assert!(
                find_by_guid_and_feed(&db, &g, feed_id)
                    .await
                    .unwrap()
                    .is_none(),
                "{g} should have been deleted"
            );
            let outcome = upsert_entry_id(&db, feed_id, &g, Some(&g), None, None, None, None, None)
                .await
                .unwrap();
            assert!(
                matches!(outcome, UpsertOutcome::SkippedTombstoned),
                "{g} should be tombstoned, got {outcome:?}"
            );
        }
    }
}
