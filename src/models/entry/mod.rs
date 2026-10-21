use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};

use crate::db::{Db, DbInner, Tx};
use crate::error::{AppError, AppResult};
use crate::utils::text::strip_to_search_text;
use crate::{db_execute, query_opt, query_opt_tx, query_scalar};

mod filters;
pub mod query;
use filters::{
    Bind, Dialect, apply_continuation_condition, apply_filter_conditions, apply_time_conditions,
    published_sort_entry_hint,
};
// Tests only; production reaches it via `published_sort_entry_hint`.
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
    /// `entry_summary.created_at DESC`; queries join `entry_summary` via [`sort_join`].
    SummarizedAt,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub id: i64,
    pub feed_id: i64,
    pub guid: String,
    pub title: Option<String>,
    pub link: Option<String>,
    pub content: Option<String>,
    /// Raw HTML from "Fetch Full Content". Sanitised per render, never at write
    /// time: sanitising signs image-proxy URLs, and a stored signature would outlive its key.
    pub full_content: Option<String>,
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

impl EntryWithFeed {
    /// Base URL for resolving relative URLs: `entry.link`, then the feed site,
    /// then the feed URL. Never `None`, so [`crate::services::sanitize_html`]
    /// can drop unresolvable image URLs instead of passing them through unproxied.
    pub fn content_base_url(&self) -> &str {
        [self.entry.link.as_deref(), self.site_url.as_deref()]
            .into_iter()
            .flatten()
            .find(|s| !s.is_empty())
            .unwrap_or(&self.feed_url)
    }
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
    /// With `unread_only`, entries read at-or-after this UTC `YYYY-MM-DD HH:MM:SS`
    /// instant still count as unread, so reading-pane navigation can return to them.
    pub read_after: Option<String>,
    /// Boolean query AST for `/search`; `None` elsewhere. Rendered by `filters::render_query`.
    #[serde(skip)]
    pub query: Option<query::QueryNode>,
}

/// Pagination cursor: emits `<iso_8601_ts>|<id>`; also accepts a legacy bare
/// `i64` for in-flight cursors from older clients.
#[derive(Debug, Clone)]
pub enum ContinuationCursor {
    /// `(sort_ts, id)`; `sort_ts` is the sort column as stored TEXT, so the
    /// predicate compares against the indexed column without conversion.
    Composite { sort_ts: String, id: i64 },
    /// Legacy `e.id < ?` cursor; input only.
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
    /// Oldest timestamp (epoch seconds)
    pub ot: Option<i64>,
    /// Newest timestamp (epoch seconds)
    pub nt: Option<i64>,
    /// Sort order (default: `PublishedAt`)
    pub sort_order: EntrySortOrder,
}

/// Which entries [`list_by_user`] returns, in what order, and which page of them.
#[derive(Debug, Clone, Copy)]
pub struct EntryListQuery<'a> {
    pub user_id: i64,
    pub filter: &'a EntryFilter,
    pub sort_order: EntrySortOrder,
    pub limit: i64,
    pub offset: i64,
}

/// Flat row for the `EntryWithFeed` join. `FromRow` matches by column name and
/// the join has duplicate names, so `ENTRY_WITH_FEED_COLUMNS_*` alias every column.
#[derive(sqlx::FromRow)]
struct EntryWithFeedRow {
    id: i64,
    feed_id: i64,
    guid: String,
    title: Option<String>,
    link: Option<String>,
    content: Option<String>,
    full_content: Option<String>,
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

/// [`EntryWithFeedRow`] plus its continuation-cursor timestamp, selected as
/// `sort_ts` in the same query so the next cursor needs no second lookup.
#[derive(sqlx::FromRow)]
struct EntryWithFeedTsRow {
    #[sqlx(flatten)]
    row: EntryWithFeedRow,
    sort_ts: Option<String>,
}

impl From<EntryWithFeedRow> for EntryWithFeed {
    fn from(row: EntryWithFeedRow) -> Self {
        EntryWithFeed {
            entry: Entry {
                id: row.id,
                feed_id: row.feed_id,
                guid: row.guid,
                title: row.title,
                link: row.link,
                content: row.content,
                full_content: row.full_content,
                summary: row.summary,
                author: row.author,
                published_at: row.published_at,
                read_at: row.read_at,
                starred_at: row.starred_at,
                created_at: row.created_at,
                updated_at: row.updated_at,
            },
            feed_title: row.feed_title,
            feed_url: row.feed_url,
            site_url: row.site_url,
            category_id: row.category_id,
            category_name: row.category_name,
            feed_has_icon: row.has_icon > 0,
            custom_referrer: row.custom_referrer,
        }
    }
}

/// SELECT list for [`EntryWithFeedRow`]; `has_icon` via correlated COUNT, for
/// queries without `LEFT JOIN image`. Keep both variants and the row struct in sync.
const ENTRY_WITH_FEED_COLUMNS_COUNT: &str = "e.id AS id, e.feed_id AS feed_id, e.guid AS guid, e.title AS title, e.link AS link, e.content AS content, e.full_content AS full_content, e.summary AS summary, e.author AS author, e.published_at AS published_at, e.read_at AS read_at, e.starred_at AS starred_at, e.created_at AS created_at, e.updated_at AS updated_at, f.title AS feed_title, f.url AS feed_url, f.site_url AS site_url, c.id AS category_id, c.name AS category_name, (SELECT COUNT(*) FROM image i WHERE i.entity_type = 'feed' AND i.entity_id = f.id) AS has_icon, f.custom_referrer AS custom_referrer";

/// As [`ENTRY_WITH_FEED_COLUMNS_COUNT`], but `has_icon` from an existing `LEFT JOIN image i`.
const ENTRY_WITH_FEED_COLUMNS_JOIN: &str = "e.id AS id, e.feed_id AS feed_id, e.guid AS guid, e.title AS title, e.link AS link, e.content AS content, e.full_content AS full_content, e.summary AS summary, e.author AS author, e.published_at AS published_at, e.read_at AS read_at, e.starred_at AS starred_at, e.created_at AS created_at, e.updated_at AS updated_at, f.title AS feed_title, f.url AS feed_url, f.site_url AS site_url, c.id AS category_id, c.name AS category_name, CAST(CASE WHEN i.id IS NOT NULL THEN 1 ELSE 0 END AS BIGINT) AS has_icon, f.custom_referrer AS custom_referrer";

/// Apply a dynamic query's binds in order.
macro_rules! bind_all {
    ($q:expr, $binds:expr) => {{
        let mut q = $q;
        for b in $binds {
            q = match b {
                Bind::Int(i) => q.bind(*i),
                Bind::Text(s) => q.bind(s.as_str()),
                Bind::Ts(t) => q.bind(*t),
            };
        }
        q
    }};
}

// --- dynamic-query execution helpers ---------------------------------------
// Runtime-built SQL (`$N` placeholders + parallel `Vec<Bind>`), wrapped in
// `sqlx::AssertSqlSafe` — every fragment is built by this module.

async fn fetch_entries_with_feed(
    db: &Db,
    sql: String,
    binds: Vec<Bind>,
) -> Result<Vec<EntryWithFeed>, sqlx::Error> {
    let rows = match db.inner() {
        DbInner::Sqlite(pool) => {
            bind_all!(
                sqlx::query_as::<sqlx::Sqlite, EntryWithFeedRow>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await?
        }
        DbInner::Postgres(pool) => {
            bind_all!(
                sqlx::query_as::<sqlx::Postgres, EntryWithFeedRow>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows.into_iter().map(EntryWithFeed::from).collect())
}

async fn fetch_entries_with_feed_ts(
    db: &Db,
    sql: String,
    binds: Vec<Bind>,
) -> Result<Vec<(EntryWithFeed, Option<String>)>, sqlx::Error> {
    let rows = match db.inner() {
        DbInner::Sqlite(pool) => {
            bind_all!(
                sqlx::query_as::<sqlx::Sqlite, EntryWithFeedTsRow>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await?
        }
        DbInner::Postgres(pool) => {
            bind_all!(
                sqlx::query_as::<sqlx::Postgres, EntryWithFeedTsRow>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await?
        }
    };
    Ok(rows
        .into_iter()
        .map(|r| (EntryWithFeed::from(r.row), r.sort_ts))
        .collect())
}

async fn fetch_scalar_i64(db: &Db, sql: String, binds: Vec<Bind>) -> Result<i64, sqlx::Error> {
    match db.inner() {
        DbInner::Sqlite(pool) => {
            bind_all!(
                sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_one(pool)
            .await
        }
        DbInner::Postgres(pool) => {
            bind_all!(
                sqlx::query_scalar::<sqlx::Postgres, i64>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_one(pool)
            .await
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
            bind_all!(
                sqlx::query_as::<sqlx::Sqlite, (i64, i64)>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await
        }
        DbInner::Postgres(pool) => {
            bind_all!(
                sqlx::query_as::<sqlx::Postgres, (i64, i64)>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_all(pool)
            .await
        }
    }
}

/// Execute a runtime-built write, returning rows affected. Takes write-priority
/// admission for its duration.
async fn exec_dynamic(db: &Db, sql: String, binds: Vec<Bind>) -> Result<u64, sqlx::Error> {
    let _guard = db.admit().await;
    match db.inner() {
        DbInner::Sqlite(pool) => bind_all!(
            sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql)),
            &binds
        )
        .execute(pool)
        .await
        .map(|r| r.rows_affected()),
        DbInner::Postgres(pool) => bind_all!(
            sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(crate::db::pg_rewrite(&sql))),
            &binds
        )
        .execute(pool)
        .await
        .map(|r| r.rows_affected()),
    }
}

/// `exec_dynamic` against an open transaction.
async fn exec_dynamic_tx(
    tx: &mut Tx<'_>,
    sql: String,
    binds: Vec<Bind>,
) -> Result<u64, sqlx::Error> {
    match tx {
        Tx::Sqlite { tx: t, .. } => bind_all!(
            sqlx::query::<sqlx::Sqlite>(sqlx::AssertSqlSafe(sql)),
            &binds
        )
        .execute(&mut **t)
        .await
        .map(|r| r.rows_affected()),
        Tx::Postgres(t) => bind_all!(
            sqlx::query::<sqlx::Postgres>(sqlx::AssertSqlSafe(crate::db::pg_rewrite(&sql))),
            &binds
        )
        .execute(&mut **t)
        .await
        .map(|r| r.rows_affected()),
    }
}

pub async fn find_by_id(db: &Db, id: i64) -> AppResult<Option<Entry>> {
    query_opt!(
        db,
        Entry,
        "SELECT id, feed_id, guid, title, link, content, full_content, summary, author, \
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

/// Fetch an entry scoped to `user_id`; `None` if missing or not owned (both 404).
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

/// Sort-field value as the exact cursor TEXT; `None` if the entry doesn't exist.
/// The column a sort order pages by, over the `entry e` alias.
fn sort_column(sort_order: EntrySortOrder) -> &'static str {
    match sort_order {
        EntrySortOrder::ReadAt => "e.read_at",
        EntrySortOrder::StarredAt => "e.starred_at",
        EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at)",
        EntrySortOrder::SummarizedAt => "es.created_at",
    }
}

/// The join a sort order's column needs, scoped to user `$1`; `SummarizedAt`
/// also drops entries without a summary.
fn sort_join(sort_order: EntrySortOrder) -> &'static str {
    match sort_order {
        EntrySortOrder::SummarizedAt => {
            " INNER JOIN entry_summary es ON es.entry_id = e.id AND es.user_id = $1"
        }
        _ => "",
    }
}

/// `ORDER BY` for a paged sort: the sort column, then `e.id` as tie-breaker.
fn order_by(sort_order: EntrySortOrder, oldest_first: bool) -> String {
    let dir = if oldest_first { "ASC" } else { "DESC" };
    format!("{} {dir}, e.id {dir}", sort_column(sort_order))
}

pub async fn fetch_sort_ts(
    db: &Db,
    entry_id: i64,
    sort_order: EntrySortOrder,
) -> AppResult<Option<String>> {
    // Must match the WHERE predicate's form: raw TEXT on SQLite, `to_char` on PG.
    // See `Dialect::cursor_ts`.
    let ts_expr = Dialect::from_db(db).cursor_ts(sort_column(sort_order));
    // `$1` is the entry id here, so join on it alone: an entry has one owner.
    let join = match sort_order {
        EntrySortOrder::SummarizedAt => " INNER JOIN entry_summary es ON es.entry_id = e.id",
        _ => "",
    };
    let sql = format!("SELECT {ts_expr} FROM entry e{join} WHERE e.id = $1");
    let ts = match db.inner() {
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
    Ok(ts.flatten())
}

pub async fn find_by_guid_and_feed(db: &Db, guid: &str, feed_id: i64) -> AppResult<Option<Entry>> {
    query_opt!(
        db,
        Entry,
        "SELECT id, feed_id, guid, title, link, content, full_content, summary, author, \
         published_at, read_at, starred_at, created_at, updated_at \
         FROM entry WHERE guid = $1 AND feed_id = $2",
        guid,
        feed_id
    )
    .map_err(AppError::Database)
}

pub async fn list_by_user(db: &Db, query: &EntryListQuery<'_>) -> AppResult<Vec<EntryWithFeed>> {
    let EntryListQuery {
        user_id,
        filter,
        sort_order,
        limit,
        offset,
    } = *query;
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);

    let where_clause = conditions.join(" AND ");

    let order_by = format!("{} DESC", sort_column(sort_order));
    let sort_join = sort_join(sort_order);

    // Force the published-order index: SQLite otherwise walks
    // `category -> feed -> entry` and sorts every row. `index_hint` is a no-op on PG.
    let entry_hint = if sort_order == EntrySortOrder::PublishedAt {
        dialect.index_hint(published_sort_entry_hint(filter))
    } else {
        ""
    };

    let limit_idx = binds.len() + 1;
    let offset_idx = binds.len() + 2;
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN} \
         FROM entry e{entry_hint}{sort_join} \
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

/// Entries a client mirrors for offline reading, newest first. `keep` is the
/// total budget: unread first, then starred fill the rest. `keep == 0` returns
/// nothing without touching the database.
pub async fn list_offline_set(db: &Db, user_id: i64, keep: i64) -> AppResult<Vec<EntryWithFeed>> {
    if keep <= 0 {
        return Ok(Vec::new());
    }

    let unread = list_by_user(
        db,
        &EntryListQuery {
            user_id,
            filter: &EntryFilter {
                unread_only: true,
                ..Default::default()
            },
            sort_order: EntrySortOrder::PublishedAt,
            limit: keep,
            offset: 0,
        },
    )
    .await?;

    let mut seen: std::collections::HashSet<i64> = unread.iter().map(|e| e.entry.id).collect();
    let mut set = unread;

    let remaining = keep - set.len() as i64;
    if remaining > 0 {
        let starred = list_by_user(
            db,
            &EntryListQuery {
                user_id,
                filter: &EntryFilter {
                    starred_only: true,
                    ..Default::default()
                },
                sort_order: EntrySortOrder::StarredAt,
                limit: remaining,
                offset: 0,
            },
        )
        .await?;
        // An unread entry can also be starred, so dedupe; the set may fall short
        // of the budget by the overlap.
        set.extend(starred.into_iter().filter(|e| seen.insert(e.entry.id)));
    }

    set.sort_by(|a, b| {
        let key = |e: &EntryWithFeed| {
            (
                e.entry.published_at.unwrap_or(e.entry.created_at),
                e.entry.id,
            )
        };
        key(b).cmp(&key(a))
    });
    Ok(set)
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

/// `feed_id` -> unread count for one category's feeds (keeps lazy loading off
/// the all-feeds aggregate). `user_id` stays in the WHERE so a guessed
/// `category_id` can't read another account's counts.
pub async fn count_unread_by_feed_in_category(
    db: &Db,
    user_id: i64,
    category_id: i64,
) -> AppResult<std::collections::HashMap<i64, i64>> {
    let hint = Dialect::from_db(db).index_hint(" INDEXED BY idx_entry_unread_feed");
    let sql = format!(
        "SELECT f.id, COUNT(e.id) FROM feed f \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN entry e{hint} ON e.feed_id = f.id AND e.read_at IS NULL \
         WHERE c.user_id = $1 AND f.category_id = $2 GROUP BY f.id"
    );
    let rows = fetch_id_ts_rows(db, sql, vec![Bind::Int(user_id), Bind::Int(category_id)])
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

/// Result of an entry upsert. `SkippedTombstoned` means the guid is tombstoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Inserted(i64),
    Updated(i64),
    /// Every mutable column already matched; no write issued.
    Unchanged(i64),
    SkippedTombstoned,
}

// Upsert statements. `datetime('now')` (not a bound `Utc::now()`) keeps
// `updated_at` in the column-DEFAULT TEXT format the string-compared cursor
// needs; `pg_rewrite` maps it to `now()`.
// SELECT only `id`: `UNIQUE(feed_id, guid)` makes it index-only; reading the
// comparable columns costs more than the write it would save, so the no-op
// check lives in the UPDATE's WHERE.
const UPSERT_SELECT_SQL: &str = "SELECT id FROM entry WHERE guid = $1 AND feed_id = $2";
// The UPDATE only writes when a column differs, so re-served identical articles
// cost no write; `rows_affected() == 0` means `Unchanged`. NULL-safe inequality
// forks per dialect (`IS NOT` / `IS DISTINCT FROM`) as two literals on purpose:
// a `pg_rewrite` rule would also corrupt every `IS NOT NULL`. `content_text`
// derives from `content`, so it needn't be compared.
const UPSERT_UPDATE_SQL_SQLITE: &str = "UPDATE entry SET title = $1, link = $2, content = $3, summary = $4, author = $5, content_text = $6, updated_at = datetime('now') WHERE id = $7 AND (title IS NOT $1 OR link IS NOT $2 OR content IS NOT $3 OR summary IS NOT $4 OR author IS NOT $5)";
const UPSERT_UPDATE_SQL_PG: &str = "UPDATE entry SET title = $1, link = $2, content = $3, summary = $4, author = $5, content_text = $6, updated_at = now() WHERE id = $7 AND (title IS DISTINCT FROM $1 OR link IS DISTINCT FROM $2 OR content IS DISTINCT FROM $3 OR summary IS DISTINCT FROM $4 OR author IS DISTINCT FROM $5)";
const UPSERT_INSERT_SQL: &str = "INSERT INTO entry (feed_id, guid, title, link, content, summary, author, published_at, content_text) SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9 WHERE NOT EXISTS (SELECT 1 FROM entry_tombstone WHERE feed_id = $1 AND guid = $2) RETURNING id";

/// Mutable-column payload shared by the pooled and tx update helpers.
struct UpsertUpdate<'a> {
    id: i64,
    title: Option<&'a str>,
    link: Option<&'a str>,
    content: Option<&'a str>,
    summary: Option<&'a str>,
    author: Option<&'a str>,
    content_text: Option<&'a str>,
}

/// Guarded UPDATE on a transaction; hand-dispatched because the SQL differs per dialect.
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

/// Fields of one entry as fetched from a feed; input to the `upsert_entry*` family.
pub struct EntryUpsert<'a> {
    pub feed_id: i64,
    pub guid: &'a str,
    pub title: Option<&'a str>,
    pub link: Option<&'a str>,
    pub content: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub author: Option<&'a str>,
    pub published_at: Option<DateTime<Utc>>,
}

impl<'a> EntryUpsert<'a> {
    /// Required fields only; the optional ones start as `None`.
    pub fn new(feed_id: i64, guid: &'a str) -> Self {
        Self {
            feed_id,
            guid,
            title: None,
            link: None,
            content: None,
            summary: None,
            author: None,
            published_at: None,
        }
    }
}

/// Upsert an entry without re-reading the row (feed-sync hot path). The insert
/// is tombstone-guarded via `RETURNING id`: no row means tombstoned.
pub async fn upsert_entry_id(db: &Db, entry: &EntryUpsert<'_>) -> AppResult<UpsertOutcome> {
    let EntryUpsert {
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at,
    } = *entry;

    // Seconds-truncated NaiveDateTime: SQLite cursor TEXT, PG `timestamp`
    // (a raw string bind is rejected by PG's `timestamptz`).
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

/// Transactional [`upsert_entry_id`] for feed sync's atomic unit of work.
pub async fn upsert_entry_id_tx(
    tx: &mut Tx<'_>,
    entry: &EntryUpsert<'_>,
) -> AppResult<UpsertOutcome> {
    let EntryUpsert {
        feed_id,
        guid,
        title,
        link,
        content,
        summary,
        author,
        published_at,
    } = *entry;
    // Seconds-truncated NaiveDateTime: SQLite cursor TEXT, PG `timestamp`
    // (a raw string bind is rejected by PG's `timestamptz`).
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

/// Idempotent `(feed_id, guid)` tombstone insert.
const INSERT_TOMBSTONE_SQL: &str = "INSERT INTO entry_tombstone (feed_id, guid) VALUES ($1, $2) ON CONFLICT(feed_id, guid) DO NOTHING";

/// Record a tombstone for `(feed_id, guid)`. Idempotent.
pub async fn insert_tombstone(db: &Db, feed_id: i64, guid: &str) -> AppResult<()> {
    db_execute!(db, INSERT_TOMBSTONE_SQL, feed_id, guid).map_err(AppError::Database)?;
    Ok(())
}

/// Victim-selection SQL for retention pruning; built at call time because the
/// interval dialect-forks (see [`Dialect::days_ago`]).
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

/// Delete up to `batch_size` read, aged, unstarred entries of opted-in users,
/// tombstoning each; returns the count. One transaction per batch keeps
/// tombstone+delete atomic against a concurrent refresh.
pub async fn prune_read_retention_batch(db: &Db, batch_size: usize) -> AppResult<u64> {
    let sql = retention_victims_sql(Dialect::from_db(db));
    let mut tx = db.begin().await?;

    // Dynamic SQL can't use `query_all_tx!`, so dispatch on the backend directly.
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

    // Two set-based statements per batch (not two per victim) to shorten the
    // SQLite write-lock window. `WHERE id IN (...)` also disambiguates SQLite's
    // `INSERT ... SELECT ... ON CONFLICT` parse.
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

/// Upsert and return the full [`Entry`] plus whether it was new.
pub async fn upsert_entry(db: &Db, upsert: &EntryUpsert<'_>) -> AppResult<(Entry, bool)> {
    let (id, is_new) = match upsert_entry_id(db, upsert).await? {
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

/// Set starred state scoped to the owner. Idempotent. Returns the entry plus
/// `changed`, or `None` if missing or not owned (both 404).
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
        .map(|entry_with_feed| (entry_with_feed, changed)))
}

/// Store the raw "Fetch Full Content" HTML, scoped to the owner; overwrites any
/// previous fetch. Sanitised on read — see [`Entry::full_content`].
pub async fn set_full_content_for_user(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    html: &str,
) -> AppResult<()> {
    // Non-owned entries update nothing rather than erroring.
    db_execute!(
        db,
        "UPDATE entry SET full_content = $1, updated_at = datetime('now') \
         WHERE id = $2 AND feed_id IN ( \
             SELECT f.id FROM feed f JOIN category c ON c.id = f.category_id \
             WHERE c.user_id = $3 \
         )",
        html,
        entry_id,
        user_id
    )
    .map_err(AppError::Database)?;
    Ok(())
}

/// Set read state scoped to the owner. Idempotent. Returns the entry (`None` if
/// missing or not owned) plus whether state changed.
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
        .map(|entry_with_feed| (entry_with_feed, changed)))
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

/// List `(entry_id, timestamp_usec)` for a user with continuation-based pagination.
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
    apply_time_conditions(&mut conditions, &mut binds, pagination, dialect);
    apply_continuation_condition(&mut conditions, &mut binds, pagination, dialect);

    let where_clause = conditions.join(" AND ");
    let order = order_by(pagination.sort_order, pagination.oldest_first);
    let sort_join = sort_join(pagination.sort_order);

    let limit_idx = binds.len() + 1;
    // "No more pages" sentinel; epoch extraction dialect-forks.
    let epoch_us = dialect.epoch("COALESCE(e.published_at, e.created_at)");
    let sql = format!(
        "SELECT e.id, {epoch_us} * 1000000 \
         FROM entry e{sort_join} \
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

/// What [`continuation_query`] builds a page for; `extra_select` is appended to
/// the SELECT list.
#[derive(Clone, Copy)]
struct ContinuationQuery<'a> {
    user_id: i64,
    filter: &'a EntryFilter,
    pagination: &'a ContinuationParams,
    extra_select: &'a str,
}

/// SQL and binds for a continuation page.
fn continuation_query(db: &Db, query: &ContinuationQuery<'_>) -> (String, Vec<Bind>) {
    let ContinuationQuery {
        user_id,
        filter,
        pagination,
        extra_select,
    } = *query;
    let dialect = Dialect::from_db(db);
    let mut conditions = vec!["c.user_id = $1".to_string()];
    let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];

    apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);
    apply_time_conditions(&mut conditions, &mut binds, pagination, dialect);
    apply_continuation_condition(&mut conditions, &mut binds, pagination, dialect);

    let where_clause = conditions.join(" AND ");
    let order = order_by(pagination.sort_order, pagination.oldest_first);
    let sort_join = sort_join(pagination.sort_order);

    // Page-0 only: without the hint the planner walks category->feed->entry and
    // sorts the whole corpus. With a continuation predicate the sort index is
    // already used, so leave that plan alone.
    let entry_hint = if pagination.sort_order == EntrySortOrder::PublishedAt
        && pagination.continuation.is_none()
    {
        dialect.index_hint(published_sort_entry_hint(filter))
    } else {
        ""
    };

    let limit_idx = binds.len() + 1;
    let sql = format!(
        "SELECT {ENTRY_WITH_FEED_COLUMNS_JOIN}{extra_select} \
         FROM entry e{entry_hint}{sort_join} \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         LEFT JOIN image i ON i.entity_type = 'feed' AND i.entity_id = f.id \
         WHERE {where_clause} \
         ORDER BY {order} \
         LIMIT ${limit_idx}"
    );

    binds.push(Bind::Int(pagination.limit));
    (sql, binds)
}

/// List entries with continuation-based pagination (for Google Reader stream/contents).
pub async fn list_by_user_with_continuation(
    db: &Db,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<EntryWithFeed>> {
    let (sql, binds) = continuation_query(
        db,
        &ContinuationQuery {
            user_id,
            filter,
            pagination,
            extra_select: "",
        },
    );
    fetch_entries_with_feed(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

/// As [`list_by_user_with_continuation`], each row paired with its cursor
/// timestamp (what [`fetch_sort_ts`] would return for it).
pub async fn list_by_user_with_continuation_ts(
    db: &Db,
    user_id: i64,
    filter: &EntryFilter,
    pagination: &ContinuationParams,
) -> AppResult<Vec<(EntryWithFeed, Option<String>)>> {
    let ts_expr = Dialect::from_db(db).cursor_ts(sort_column(pagination.sort_order));
    let (sql, binds) = continuation_query(
        db,
        &ContinuationQuery {
            user_id,
            filter,
            pagination,
            extra_select: &format!(", {ts_expr} AS sort_ts"),
        },
    );
    fetch_entries_with_feed_ts(db, sql, binds)
        .await
        .map_err(AppError::Database)
}

/// The next continuation cursor for a page fetched with `page_size + 1` rows:
/// from the last kept row, or `None` when the look-ahead row is absent.
pub fn next_continuation<T>(
    rows: &[(T, Option<String>)],
    page_size: usize,
    id: impl Fn(&T) -> i64,
) -> Option<String> {
    if rows.len() <= page_size {
        return None;
    }
    let (last, ts) = rows.get(page_size.checked_sub(1)?)?;
    ts.as_deref()
        .map(|ts| ContinuationCursor::encode_composite(ts, id(last)))
}

/// Mark a feed's unread entries read, optionally only those older than
/// `older_than_days`. Returns rows affected.
pub async fn mark_all_read_by_feed(
    db: &Db,
    feed_id: i64,
    older_than_days: Option<i64>,
) -> AppResult<i64> {
    // `days` is an `i64` (not injectable); the cutoff dialect-forks via `Dialect::days_ago`.
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

/// Mark every unread entry matching `filter` and owned by `user_id` as read,
/// using the list query's filter builder. Returns rows affected.
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

/// Sort timestamp of an entry the user owns, as cursor TEXT (see
/// `Dialect::cursor_ts`); `None` when the entry is missing, not theirs, or has no
/// timestamp.
async fn fetch_sort_time(
    db: &Db,
    user_id: i64,
    entry_id: i64,
) -> Result<Option<String>, sqlx::Error> {
    let sort_ts_select = Dialect::from_db(db).cursor_ts("COALESCE(e.published_at, e.created_at)");
    let sql = format!(
        "SELECT {sort_ts_select} \
         FROM entry e \
         INNER JOIN feed f ON e.feed_id = f.id \
         INNER JOIN category c ON f.category_id = c.id \
         WHERE e.id = $1 AND c.user_id = $2"
    );
    match db.inner() {
        DbInner::Sqlite(pool) => {
            sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(sqlx::AssertSqlSafe(sql))
                .bind(entry_id)
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map(Option::flatten)
        }
        DbInner::Postgres(pool) => {
            sqlx::query_scalar::<sqlx::Postgres, Option<String>>(sqlx::AssertSqlSafe(sql))
                .bind(entry_id)
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .map(Option::flatten)
        }
    }
}

/// `filter`'s conditions as an ` AND ...` suffix (empty when there are none),
/// with their binds appended to `binds`.
fn filter_suffix(binds: &mut Vec<Bind>, filter: &EntryFilter, dialect: Dialect) -> String {
    let mut conditions = Vec::new();
    apply_filter_conditions(&mut conditions, binds, filter, dialect);
    if conditions.is_empty() {
        return String::new();
    }
    format!(" AND {}", conditions.join(" AND "))
}

async fn fetch_neighbor_id(
    db: &Db,
    sql: String,
    binds: Vec<Bind>,
) -> Result<Option<i64>, sqlx::Error> {
    match db.inner() {
        DbInner::Sqlite(pool) => {
            bind_all!(
                sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_optional(pool)
            .await
        }
        DbInner::Postgres(pool) => {
            bind_all!(
                sqlx::query_scalar::<sqlx::Postgres, i64>(sqlx::AssertSqlSafe(sql)),
                &binds
            )
            .fetch_optional(pool)
            .await
        }
    }
}

/// Prev (newer) / next (older) entry by `COALESCE(published_at, created_at)` DESC,
/// honouring every `EntryFilter` condition.
pub async fn find_neighbors(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    filter: &EntryFilter,
) -> AppResult<EntryNeighbors> {
    let dialect = Dialect::from_db(db);

    let sort_time = fetch_sort_time(db, user_id, entry_id)
        .await
        .map_err(AppError::Database)?;

    let Some(sort_time) = sort_time else {
        return Ok(EntryNeighbors {
            prev_id: None,
            next_id: None,
        });
    };

    // On PG bind a `timestamptz` so the lookup is an index range scan; fall back
    // to the `to_char` string if unparseable. SQLite compares raw TEXT.
    let pg_ts = (dialect == Dialect::Postgres)
        .then(|| filters::parse_cursor_ts(&sort_time))
        .flatten();
    let cursor_bind = |ts: Option<chrono::DateTime<chrono::Utc>>, s: &str| match ts {
        Some(ts) => Bind::Ts(ts),
        None => Bind::Text(s.to_string()),
    };

    // Prev binds: $1=user_id, $2=sort_time
    let mut prev_binds: Vec<Bind> = vec![Bind::Int(user_id), cursor_bind(pg_ts, &sort_time)];
    let prev_extra = filter_suffix(&mut prev_binds, filter, dialect);

    // Next binds: $1=user_id, $2=sort_time, $3=entry_id
    let mut next_binds: Vec<Bind> = vec![
        Bind::Int(user_id),
        cursor_bind(pg_ts, &sort_time),
        Bind::Int(entry_id),
    ];
    let next_extra = filter_suffix(&mut next_binds, filter, dialect);

    // Pin the sort index with entry driving the join so it stops at LIMIT 1.
    // Only for the snapshot OR `(read_at IS NULL OR read_at >= ?)`, which
    // otherwise gets a MULTI-INDEX OR plan that is O(table). The strict unread
    // path must not be pinned: it uses the partial `idx_entry_unread_feed`.
    // `index_hint` is a no-op on PG.
    let (raw_hint, join_kw) = if filter.unread_only && filter.read_after.is_some() {
        (" INDEXED BY idx_entry_sort_ts", "CROSS JOIN")
    } else {
        (published_sort_entry_hint(filter), "INNER JOIN")
    };
    let entry_hint = dialect.index_hint(raw_hint);

    // Compare in the same form as the `$2` bind (raw column, or `to_char` on the
    // PG string fallback); ORDER BY always uses the raw expression.
    let cmp_ts = match pg_ts {
        Some(_) => "COALESCE(e.published_at, e.created_at)".to_string(),
        None => dialect.cursor_ts("COALESCE(e.published_at, e.created_at)"),
    };

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

    let prev_id = fetch_neighbor_id(db, prev_sql, prev_binds)
        .await
        .map_err(AppError::Database)?;
    let next_id = fetch_neighbor_id(db, next_sql, next_binds)
        .await
        .map_err(AppError::Database)?;

    Ok(EntryNeighbors { prev_id, next_id })
}

/// Counting stops here, so the cost stays flat on huge lists.
pub const POSITION_COUNT_LIMIT: i64 = 1000;

/// 1-based place of an entry in a filtered list, and the list's size. Each is
/// capped at `limit + 1`; a value above `limit` means "more than `limit`".
#[derive(Debug, Clone, Serialize)]
pub struct EntryPosition {
    pub position: i64,
    pub total: i64,
    pub limit: i64,
}

/// Where `entry_id` sits in the `published DESC, id DESC` order under `filter`:
/// the entries ahead of it (itself included), against the same sort key
/// `find_neighbors` walks. Counts are capped at [`POSITION_COUNT_LIMIT`] + 1.
pub async fn find_position(
    db: &Db,
    user_id: i64,
    entry_id: i64,
    filter: &EntryFilter,
) -> AppResult<EntryPosition> {
    let dialect = Dialect::from_db(db);

    async fn count(db: &Db, sql: String, binds: Vec<Bind>) -> Result<i64, sqlx::Error> {
        match db.inner() {
            DbInner::Sqlite(pool) => {
                bind_all!(
                    sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(sql)),
                    &binds
                )
                .fetch_one(pool)
                .await
            }
            DbInner::Postgres(pool) => {
                bind_all!(
                    sqlx::query_scalar::<sqlx::Postgres, i64>(sqlx::AssertSqlSafe(sql)),
                    &binds
                )
                .fetch_one(pool)
                .await
            }
        }
    }

    // `$1` is the user id and, when `by_entry`, `$2` the entry whose sort key
    // `cmp` compares against; the filter's binds follow.
    let capped_count = |by_entry: bool, cmp: &str| {
        let mut conditions = Vec::new();
        let mut binds: Vec<Bind> = vec![Bind::Int(user_id)];
        if by_entry {
            binds.push(Bind::Int(entry_id));
        }
        apply_filter_conditions(&mut conditions, &mut binds, filter, dialect);
        let extra = if conditions.is_empty() {
            String::new()
        } else {
            format!(" AND {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT COUNT(*) FROM ( \
                 SELECT 1 FROM entry e \
                 INNER JOIN feed f ON e.feed_id = f.id \
                 INNER JOIN category c ON f.category_id = c.id \
                 WHERE c.user_id = $1{cmp}{extra} \
                 LIMIT {} \
             ) capped",
            POSITION_COUNT_LIMIT + 1
        );
        (sql, binds)
    };

    let ts = "COALESCE(e.published_at, e.created_at)";
    let own_ts = "(SELECT COALESCE(s.published_at, s.created_at) FROM entry s WHERE s.id = $2)";

    let (sql, binds) = capped_count(false, "");
    let total = count(db, sql, binds).await.map_err(AppError::Database)?;

    // Split the tie-break from the range so the sort index can serve the range
    // half; an OR across both would force a scan.
    let (sql, binds) = capped_count(true, &format!(" AND {ts} > {own_ts}"));
    let ahead = count(db, sql, binds).await.map_err(AppError::Database)?;
    let (sql, binds) = capped_count(true, &format!(" AND {ts} = {own_ts} AND e.id >= $2"));
    let tied = count(db, sql, binds).await.map_err(AppError::Database)?;

    Ok(EntryPosition {
        position: (ahead + tied).min(POSITION_COUNT_LIMIT + 1),
        total,
        limit: POSITION_COUNT_LIMIT,
    })
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

/// A by-ids bulk update: `SET` `set_clause` on `entry_ids` owned by `user_id`;
/// `extra_where` is appended after `id IN (...)`.
#[derive(Clone, Copy)]
struct BulkUpdate<'a> {
    user_id: i64,
    entry_ids: &'a [i64],
    set_clause: &'a str,
    extra_where: &'a str,
}

/// Bulk-update SQL + binds for by-ids operations. `$1` is the user id; ids fill
/// `$2..`.
fn build_update_by_ids(update: &BulkUpdate<'_>) -> (String, Vec<Bind>) {
    let BulkUpdate {
        user_id,
        entry_ids,
        set_clause,
        extra_where,
    } = *update;
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

/// Apply `SET` to `entry_ids` owned by the user in one statement; returns rows
/// updated (0 for empty input).
async fn update_entries_by_ids(db: &Db, update: &BulkUpdate<'_>) -> AppResult<i64> {
    if update.entry_ids.is_empty() {
        return Ok(0);
    }
    let (sql, binds) = build_update_by_ids(update);
    let rows = exec_dynamic(db, sql, binds)
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Transactional [`update_entries_by_ids`] for `GReader` `edit_tag`.
async fn update_entries_by_ids_tx(tx: &mut Tx<'_>, update: &BulkUpdate<'_>) -> AppResult<i64> {
    if update.entry_ids.is_empty() {
        return Ok(0);
    }
    let (sql, binds) = build_update_by_ids(update);
    let rows = exec_dynamic_tx(tx, sql, binds)
        .await
        .map_err(AppError::Database)?;
    Ok(rows as i64)
}

/// Bulk mark currently-unread entries read, scoped to the user.
pub async fn mark_read_by_ids(db: &Db, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids(
        db,
        &BulkUpdate {
            user_id,
            entry_ids,
            set_clause: "read_at = datetime('now'), updated_at = datetime('now')",
            extra_where: " AND read_at IS NULL",
        },
    )
    .await
}

/// Bulk mark unread, scoped to the user, in `edit_tag`'s transaction.
pub async fn mark_unread_by_ids_tx(
    tx: &mut Tx<'_>,
    user_id: i64,
    entry_ids: &[i64],
) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        &BulkUpdate {
            user_id,
            entry_ids,
            set_clause: "read_at = NULL, updated_at = datetime('now')",
            extra_where: "",
        },
    )
    .await
}

/// Bulk star not-yet-starred entries, scoped to the user, in `edit_tag`'s transaction.
pub async fn star_by_ids_tx(tx: &mut Tx<'_>, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        &BulkUpdate {
            user_id,
            entry_ids,
            set_clause: "starred_at = datetime('now'), updated_at = datetime('now')",
            extra_where: " AND starred_at IS NULL",
        },
    )
    .await
}

/// Bulk unstar, scoped to the user, in `edit_tag`'s transaction.
pub async fn unstar_by_ids_tx(tx: &mut Tx<'_>, user_id: i64, entry_ids: &[i64]) -> AppResult<i64> {
    update_entries_by_ids_tx(
        tx,
        &BulkUpdate {
            user_id,
            entry_ids,
            set_clause: "starred_at = NULL, updated_at = datetime('now')",
            extra_where: "",
        },
    )
    .await
}

#[cfg(test)]
mod tests;
