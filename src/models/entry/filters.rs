//! WHERE-clause and index-hint builders for the entry list and continuation
//! queries. Pure SQL-fragment construction over the `EntryFilter` / cursor
//! types — no database access — extracted from `entry/mod.rs` to keep the
//! query-shaping logic in one focused place.
//!
//! These build a SQL string with `$N` placeholders plus a parallel `Vec<Bind>`
//! of values, applied positionally at execution time (see `entry/mod.rs`). The
//! backend `Dialect` selects the few divergent fragments (case-insensitive
//! `LIKE`, epoch extraction, the SQLite-only `INDEXED BY` hint).

use chrono::{DateTime, Utc};

use crate::db::Db;

use super::{ContinuationCursor, EntryFilter, EntrySortOrder};

/// A positional bind value for the dynamically-built entry queries. Applied in
/// order against the concrete-backend query at execution (`$1`, `$2`, ...).
pub(super) enum Bind {
    Int(i64),
    Text(String),
    /// A timestamp bound as the backend's native type (`timestamptz` on PG,
    /// `%Y-%m-%d %H:%M:%S` TEXT on `SQLite`). Used for the cursor / `read_after`
    /// comparisons so they hit the timestamp index as a range scan instead of
    /// being filtered through a non-sargable `to_char(...)` expression.
    Ts(DateTime<Utc>),
}

/// Parse the `%Y-%m-%d %H:%M:%S` cursor TEXT (as emitted by
/// [`super::fetch_sort_ts`], UTC) into a `timestamptz`-bindable value. Returns
/// `None` for a malformed cursor (e.g. a tampered wire value), letting callers
/// fall back to the string comparison.
pub(super) fn parse_cursor_ts(s: &str) -> Option<DateTime<Utc>> {
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|ndt| ndt.and_utc())
}

/// Backend SQL-dialect selector for the entry queries. Derived once from the
/// live [`Db`] and threaded through the fragment builders.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Dialect {
    Sqlite,
    Postgres,
}

impl Dialect {
    pub(super) fn from_db(db: &Db) -> Self {
        match db {
            Db::Sqlite(_) => Dialect::Sqlite,
            Db::Postgres(_) => Dialect::Postgres,
        }
    }

    /// Case-insensitive `LIKE`: `LIKE ... COLLATE NOCASE` on `SQLite`, `ILIKE` on
    /// `PostgreSQL`. Returns the full operator-plus-suffix so callers write
    /// `e.title {like} $n` where `{like}` already carries any trailing collate.
    fn ci_like(self, column: &str, placeholder: usize) -> String {
        match self {
            Dialect::Sqlite => format!("{column} LIKE ${placeholder} COLLATE NOCASE"),
            Dialect::Postgres => format!("{column} ILIKE ${placeholder}"),
        }
    }

    /// Unix-epoch-seconds expression for a timestamp column/expression:
    /// `CAST(strftime('%s', expr) AS INTEGER)` on `SQLite`,
    /// `EXTRACT(EPOCH FROM expr)::bigint` on `PostgreSQL`.
    pub(super) fn epoch(self, expr: &str) -> String {
        match self {
            Dialect::Sqlite => format!("CAST(strftime('%s', {expr}) AS INTEGER)"),
            Dialect::Postgres => format!("EXTRACT(EPOCH FROM {expr})::bigint"),
        }
    }

    /// Render a timestamp column/expression as the `%Y-%m-%d %H:%M:%S` TEXT.
    /// On `SQLite` the columns already store exactly that TEXT, so the
    /// expression passes through. On `PostgreSQL` the columns are `TIMESTAMPTZ`,
    /// so wrap in `to_char(expr, 'YYYY-MM-DD HH24:MI:SS')`; under the pinned
    /// `TimeZone=UTC` this reproduces the string `SQLite` stores.
    ///
    /// Used for reading a timestamp column back *as* the cursor string
    /// (`fetch_sort_ts`, neighbour lookup) and as the non-sargable **fallback**
    /// for the cursor/`read_after` WHERE comparison when the bound value can't be
    /// parsed to a `timestamptz`. The fast path binds the value as a `timestamptz`
    /// (`Bind::Ts`) and compares the raw column so the timestamp index drives a
    /// range scan — see `parse_cursor_ts` and `apply_continuation_condition`.
    pub(super) fn cursor_ts(self, expr: &str) -> String {
        match self {
            Dialect::Sqlite => expr.to_string(),
            Dialect::Postgres => format!("to_char({expr}, 'YYYY-MM-DD HH24:MI:SS')"),
        }
    }

    /// A timestamp `N` days in the past, where `N` is given by the SQL
    /// expression `days_expr` (an integer literal like `30`, or an integer
    /// column like `us.retention_read_days`). Used by the read-retention and
    /// snapshot-window predicates, which compare a stored timestamp against this
    /// cutoff. `SQLite` builds it with `datetime('now', '-N days')` (concatenating
    /// the count in); `PostgreSQL` uses `now() - make_interval(days => N)`.
    pub(super) fn days_ago(self, days_expr: &str) -> String {
        match self {
            Dialect::Sqlite => format!("datetime('now', '-' || ({days_expr}) || ' days')"),
            Dialect::Postgres => format!("now() - make_interval(days => ({days_expr})::int)"),
        }
    }

    /// A query-planner index hint. `SQLite` honours `INDEXED BY`; `PostgreSQL` has
    /// no per-query hint, so the fragment collapses to empty.
    pub(super) fn index_hint(self, sqlite_hint: &'static str) -> &'static str {
        match self {
            Dialect::Sqlite => sqlite_hint,
            Dialect::Postgres => "",
        }
    }
}

pub(super) fn is_no_entry_side_predicate(filter: &EntryFilter) -> bool {
    filter.feed_id.is_none()
        && filter.category_id.is_none()
        && !filter.unread_only
        && !filter.starred_only
        && !filter.read_only
        && filter.search.is_none()
        && filter.has_summary.is_none()
}

/// `SQLite` index hint (a leading `" INDEXED BY ..."` fragment, or `""`) for
/// queries that `ORDER BY COALESCE(published_at, created_at)`. Shared by
/// `list_by_user` and `find_neighbors` so both pin the same index for the
/// published-order pages. Without it the planner walks `category -> feed ->
/// entry` over every row on a single-user instance that owns ~100% of entries.
/// Each branch maps to a partial/sort index added in schema migrations v4/v5:
/// `idx_entry_starred_sort` / `idx_entry_read_sort` for the filtered list
/// pages, `idx_entry_sort_ts` for the unfiltered case.
///
/// Returns the raw `SQLite` hint; callers pass it through [`Dialect::index_hint`]
/// so `PostgreSQL` drops it.
pub(super) fn published_sort_entry_hint(filter: &EntryFilter) -> &'static str {
    if filter.starred_only {
        " INDEXED BY idx_entry_starred_sort"
    } else if filter.read_only {
        " INDEXED BY idx_entry_read_sort"
    } else if filter.unread_only && filter.read_after.is_none() {
        // Strict unread only. The snapshot case (read_after set) widens the
        // predicate to `(read_at IS NULL OR read_at >= ?)`, which a
        // `WHERE read_at IS NULL` partial index does not cover — leave it to
        // its existing plan.
        " INDEXED BY idx_entry_unread_sort"
    } else if is_no_entry_side_predicate(filter) {
        " INDEXED BY idx_entry_sort_ts"
    } else {
        ""
    }
}

pub(super) fn apply_filter_conditions(
    conditions: &mut Vec<String>,
    binds: &mut Vec<Bind>,
    filter: &EntryFilter,
    dialect: Dialect,
) {
    if let Some(feed_id) = filter.feed_id {
        conditions.push(format!("e.feed_id = ${}", binds.len() + 1));
        binds.push(Bind::Int(feed_id));
    }

    if let Some(category_id) = filter.category_id {
        conditions.push(format!("c.id = ${}", binds.len() + 1));
        binds.push(Bind::Int(category_id));
    }

    if filter.unread_only {
        if let Some(ref read_after) = filter.read_after {
            // Snapshot semantics: entries read during the current page view
            // (read_at at-or-after the page's render instant) stay in the
            // unread navigation set. `>=` (not `>`) so a same-second
            // open-after-load still counts as in-snapshot. `read_after` is the
            // `%Y-%m-%d %H:%M:%S` snapshot string; on PG bind it as a
            // `timestamptz` and compare the raw column (sargable), falling back
            // to `to_char` if unparseable. SQLite compares raw TEXT directly.
            let idx = binds.len() + 1;
            let pg_ts = (dialect == Dialect::Postgres)
                .then(|| parse_cursor_ts(read_after))
                .flatten();
            if let Some(ts) = pg_ts {
                conditions.push(format!("(e.read_at IS NULL OR e.read_at >= ${idx})"));
                binds.push(Bind::Ts(ts));
            } else {
                let read_at = dialect.cursor_ts("e.read_at");
                conditions.push(format!("(e.read_at IS NULL OR {read_at} >= ${idx})"));
                binds.push(Bind::Text(read_after.clone()));
            }
        } else {
            conditions.push("e.read_at IS NULL".to_string());
        }
    }

    if filter.starred_only {
        conditions.push("e.starred_at IS NOT NULL".to_string());
    }

    if filter.read_only {
        conditions.push("e.read_at IS NOT NULL".to_string());
    }

    if let Some(ref search) = filter.search {
        let search_pattern = format!("%{search}%");
        let idx = binds.len() + 1;
        // Both title and content_text match the SAME single bound pattern.
        conditions.push(format!(
            "({} OR {})",
            dialect.ci_like("e.title", idx),
            dialect.ci_like("e.content_text", idx)
        ));
        binds.push(Bind::Text(search_pattern));
    }

    if let Some(has_summary) = filter.has_summary {
        // `$1` is the user_id, bound first by every caller (invariant).
        if has_summary {
            conditions.push(
                "EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = $1 AND es.entry_id = e.id)".to_string()
            );
        } else {
            conditions.push(
                "NOT EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = $1 AND es.entry_id = e.id)".to_string()
            );
        }
    }
}

/// Apply time range conditions (ot = oldest timestamp, nt = newest timestamp, in seconds).
pub(super) fn apply_time_conditions(
    conditions: &mut Vec<String>,
    binds: &mut Vec<Bind>,
    ot: Option<i64>,
    nt: Option<i64>,
    dialect: Dialect,
) {
    let epoch = dialect.epoch("COALESCE(e.published_at, e.created_at)");
    if let Some(oldest_ts) = ot {
        conditions.push(format!("{epoch} >= ${}", binds.len() + 1));
        binds.push(Bind::Int(oldest_ts));
    }

    if let Some(newest_ts) = nt {
        conditions.push(format!("{epoch} <= ${}", binds.len() + 1));
        binds.push(Bind::Int(newest_ts));
    }
}

/// Apply continuation-based pagination condition.
///
/// Composite cursor uses the V2 bounded-OR form, which the `SQLite` planner
/// can convert to an indexed range scan even when `sort_ts` is an expression
/// (`COALESCE(...)`). See `PoC` at `docs/superpowers/specs/2026-04-26-composite-cursor-pagination-design.md`.
pub(super) fn apply_continuation_condition(
    conditions: &mut Vec<String>,
    binds: &mut Vec<Bind>,
    continuation: Option<&ContinuationCursor>,
    sort_order: EntrySortOrder,
    oldest_first: bool,
    dialect: Dialect,
) {
    let Some(cursor) = continuation else {
        return;
    };

    match cursor {
        ContinuationCursor::Composite { sort_ts, id } => {
            let sort_ts_expr = match sort_order {
                EntrySortOrder::ReadAt => "e.read_at",
                EntrySortOrder::StarredAt => "e.starred_at",
                EntrySortOrder::PublishedAt => "COALESCE(e.published_at, e.created_at)",
            };
            // Prefer a sargable comparison: on PG bind the cursor as a
            // `timestamptz` and compare the RAW timestamp column so the planner
            // uses the timestamp index as a range scan (a `to_char(col)`
            // predicate is not sargable — it filters half the table at depth).
            // Fall back to the correct-but-slower `to_char` string comparison if
            // the cursor can't be parsed. SQLite compares the raw TEXT column
            // against the cursor string directly (`cursor_ts` is a no-op there).
            let pg_ts = (dialect == Dialect::Postgres)
                .then(|| parse_cursor_ts(sort_ts))
                .flatten();
            let (cmp_outer, cmp_inner) = if oldest_first {
                (">=", ">")
            } else {
                ("<=", "<")
            };
            let expr = match pg_ts {
                Some(_) => sort_ts_expr.to_string(),
                None => dialect.cursor_ts(sort_ts_expr),
            };
            let ts1 = binds.len() + 1;
            let ts2 = binds.len() + 2;
            let id_idx = binds.len() + 3;
            conditions.push(format!(
                "{expr} {cmp_outer} ${ts1} AND ({expr} {cmp_inner} ${ts2} OR e.id {cmp_inner} ${id_idx})",
            ));
            if let Some(ts) = pg_ts {
                binds.push(Bind::Ts(ts));
                binds.push(Bind::Ts(ts));
            } else {
                binds.push(Bind::Text(sort_ts.clone()));
                binds.push(Bind::Text(sort_ts.clone()));
            }
            binds.push(Bind::Int(*id));
        }
        ContinuationCursor::LegacyId(id) => {
            let cmp = if oldest_first { ">" } else { "<" };
            conditions.push(format!("e.id {} ${}", cmp, binds.len() + 1));
            binds.push(Bind::Int(*id));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unread_hint_uses_unread_sort_index() {
        let filter = EntryFilter {
            unread_only: true,
            read_after: None,
            ..Default::default()
        };
        assert_eq!(
            published_sort_entry_hint(&filter),
            " INDEXED BY idx_entry_unread_sort"
        );
    }

    #[test]
    fn test_unread_snapshot_gets_no_unread_sort_hint() {
        // Snapshot OR predicate is not covered by a WHERE read_at IS NULL index.
        let filter = EntryFilter {
            unread_only: true,
            read_after: Some("2026-01-01 00:00:00".to_string()),
            ..Default::default()
        };
        assert_ne!(
            published_sort_entry_hint(&filter),
            " INDEXED BY idx_entry_unread_sort"
        );
    }

    #[test]
    fn postgres_dialect_drops_index_hint() {
        assert_eq!(
            Dialect::Postgres.index_hint(" INDEXED BY idx_entry_sort_ts"),
            ""
        );
        assert_eq!(
            Dialect::Sqlite.index_hint(" INDEXED BY idx_entry_sort_ts"),
            " INDEXED BY idx_entry_sort_ts"
        );
    }

    fn composite(sort_ts: &str, id: i64) -> ContinuationCursor {
        ContinuationCursor::Composite {
            sort_ts: sort_ts.to_string(),
            id,
        }
    }

    fn build_continuation(cursor: &ContinuationCursor, dialect: Dialect) -> (String, Vec<Bind>) {
        let mut conditions = Vec::new();
        let mut binds = Vec::new();
        apply_continuation_condition(
            &mut conditions,
            &mut binds,
            Some(cursor),
            EntrySortOrder::PublishedAt,
            false,
            dialect,
        );
        (conditions.join(" "), binds)
    }

    // Phase D sargability contract: a valid cursor on PG must compare the RAW
    // timestamp column (index range scan) and bind a `timestamptz`, never wrap
    // the column in a non-sargable `to_char(...)`.
    #[test]
    fn pg_cursor_is_sargable_raw_timestamptz() {
        let (cond, binds) =
            build_continuation(&composite("2026-07-07 12:00:00", 42), Dialect::Postgres);
        assert!(
            !cond.contains("to_char"),
            "PG cursor must be sargable: {cond}"
        );
        assert!(cond.contains("COALESCE(e.published_at, e.created_at)"));
        assert!(matches!(binds[0], Bind::Ts(_)));
        assert!(matches!(binds[1], Bind::Ts(_)));
        assert!(matches!(binds[2], Bind::Int(42)));
    }

    // A malformed/tampered cursor can't parse → fall back to the correct (but
    // non-sargable) to_char string comparison rather than erroring.
    #[test]
    fn pg_cursor_falls_back_to_to_char_when_unparseable() {
        let (cond, binds) = build_continuation(&composite("not-a-timestamp", 1), Dialect::Postgres);
        assert!(
            cond.contains("to_char"),
            "unparseable cursor uses to_char: {cond}"
        );
        assert!(matches!(binds[0], Bind::Text(_)));
    }

    // SQLite stores timestamps as TEXT, so it compares the raw column against
    // the cursor string directly — no to_char, no Ts bind.
    #[test]
    fn sqlite_cursor_uses_raw_text() {
        let (cond, binds) =
            build_continuation(&composite("2026-07-07 12:00:00", 1), Dialect::Sqlite);
        assert!(!cond.contains("to_char"));
        assert!(matches!(binds[0], Bind::Text(_)));
    }
}
