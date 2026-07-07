//! WHERE-clause and index-hint builders for the entry list and continuation
//! queries. Pure SQL-fragment construction over the `EntryFilter` / cursor
//! types — no database access — extracted from `entry/mod.rs` to keep the
//! query-shaping logic in one focused place.
//!
//! These build a SQL string with `$N` placeholders plus a parallel `Vec<Bind>`
//! of values, applied positionally at execution time (see `entry/mod.rs`). The
//! backend `Dialect` selects the few divergent fragments (case-insensitive
//! `LIKE`, epoch extraction, the SQLite-only `INDEXED BY` hint).

use crate::db::Db;

use super::{ContinuationCursor, EntryFilter, EntrySortOrder};

/// A positional bind value for the dynamically-built entry queries. Applied in
/// order against the concrete-backend query at execution (`$1`, `$2`, ...).
pub(super) enum Bind {
    Int(i64),
    Text(String),
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
    fn epoch(self, expr: &str) -> String {
        match self {
            Dialect::Sqlite => format!("CAST(strftime('%s', {expr}) AS INTEGER)"),
            Dialect::Postgres => format!("EXTRACT(EPOCH FROM {expr})::bigint"),
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
            // open-after-load still counts as in-snapshot.
            let idx = binds.len() + 1;
            conditions.push(format!("(e.read_at IS NULL OR e.read_at >= ${idx})"));
            binds.push(Bind::Text(read_after.clone()));
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
            let (cmp_outer, cmp_inner) = if oldest_first {
                (">=", ">")
            } else {
                ("<=", "<")
            };
            let ts1 = binds.len() + 1;
            let ts2 = binds.len() + 2;
            let id_idx = binds.len() + 3;
            conditions.push(format!(
                "{expr} {cmp_outer} ${ts1} AND ({expr} {cmp_inner} ${ts2} OR e.id {cmp_inner} ${id_idx})",
                expr = sort_ts_expr,
            ));
            binds.push(Bind::Text(sort_ts.clone()));
            binds.push(Bind::Text(sort_ts.clone()));
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
}
