//! WHERE-clause and index-hint builders for the entry list and continuation
//! queries. Pure SQL-fragment construction over the `EntryFilter` / cursor
//! types — no database access — extracted from `entry/mod.rs` to keep the
//! query-shaping logic in one focused place.

use super::{ContinuationCursor, EntryFilter, EntrySortOrder};

pub(super) fn is_no_entry_side_predicate(filter: &EntryFilter) -> bool {
    filter.feed_id.is_none()
        && filter.category_id.is_none()
        && !filter.unread_only
        && !filter.starred_only
        && !filter.read_only
        && filter.search.is_none()
        && filter.has_summary.is_none()
}

/// Index hint (a leading `" INDEXED BY ..."` fragment, or `""`) for queries
/// that `ORDER BY COALESCE(published_at, created_at)`. Shared by `list_by_user`
/// and `find_neighbors` so both pin the same index for the published-order
/// pages. Without it the planner walks `category -> feed -> entry` over every
/// row on a single-user instance that owns ~100% of entries. Each branch maps
/// to a partial/sort index added in schema migrations v4/v5:
/// `idx_entry_starred_sort` / `idx_entry_read_sort` for the filtered list
/// pages, `idx_entry_sort_ts` for the unfiltered case.
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
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    filter: &EntryFilter,
) {
    if let Some(feed_id) = filter.feed_id {
        conditions.push(format!("e.feed_id = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(feed_id));
    }

    if let Some(category_id) = filter.category_id {
        conditions.push(format!("c.id = ?{}", params_vec.len() + 1));
        params_vec.push(Box::new(category_id));
    }

    if filter.unread_only {
        if let Some(ref read_after) = filter.read_after {
            // Snapshot semantics: entries read during the current page view
            // (read_at at-or-after the page's render instant) stay in the
            // unread navigation set. `>=` (not `>`) so a same-second
            // open-after-load still counts as in-snapshot.
            let idx = params_vec.len() + 1;
            conditions.push(format!("(e.read_at IS NULL OR e.read_at >= ?{})", idx));
            params_vec.push(Box::new(read_after.clone()));
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
        let search_pattern = format!("%{}%", search);
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "(e.title LIKE ?{} COLLATE NOCASE OR e.content LIKE ?{} COLLATE NOCASE)",
            param_idx, param_idx
        ));
        params_vec.push(Box::new(search_pattern));
    }

    if let Some(has_summary) = filter.has_summary {
        if has_summary {
            conditions.push(
                "EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = ?1 AND es.entry_id = e.id)".to_string()
            );
        } else {
            conditions.push(
                "NOT EXISTS (SELECT 1 FROM entry_summary es WHERE es.user_id = ?1 AND es.entry_id = e.id)".to_string()
            );
        }
    }
}

/// Apply time range conditions (ot = oldest timestamp, nt = newest timestamp, in seconds).
pub(super) fn apply_time_conditions(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
    ot: Option<i64>,
    nt: Option<i64>,
) {
    if let Some(oldest_ts) = ot {
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) >= ?{}",
            param_idx
        ));
        params_vec.push(Box::new(oldest_ts));
    }

    if let Some(newest_ts) = nt {
        let param_idx = params_vec.len() + 1;
        conditions.push(format!(
            "CAST(strftime('%s', COALESCE(e.published_at, e.created_at)) AS INTEGER) <= ?{}",
            param_idx
        ));
        params_vec.push(Box::new(newest_ts));
    }
}

/// Apply continuation-based pagination condition.
///
/// Composite cursor uses the V2 bounded-OR form, which the `SQLite` planner
/// can convert to an indexed range scan even when `sort_ts` is an expression
/// (`COALESCE(...)`). See `PoC` at `docs/superpowers/specs/2026-04-26-composite-cursor-pagination-design.md`.
pub(super) fn apply_continuation_condition(
    conditions: &mut Vec<String>,
    params_vec: &mut Vec<Box<dyn rusqlite::ToSql>>,
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
            let ts1 = params_vec.len() + 1;
            let ts2 = params_vec.len() + 2;
            let id_idx = params_vec.len() + 3;
            conditions.push(format!(
                "{expr} {cmp_outer} ?{ts1} AND ({expr} {cmp_inner} ?{ts2} OR e.id {cmp_inner} ?{id_idx})",
                expr = sort_ts_expr,
                cmp_outer = cmp_outer,
                cmp_inner = cmp_inner,
                ts1 = ts1,
                ts2 = ts2,
                id_idx = id_idx,
            ));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(sort_ts.clone()));
            params_vec.push(Box::new(*id));
        }
        ContinuationCursor::LegacyId(id) => {
            let cmp = if oldest_first { ">" } else { "<" };
            let id_idx = params_vec.len() + 1;
            conditions.push(format!("e.id {} ?{}", cmp, id_idx));
            params_vec.push(Box::new(*id));
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
}
