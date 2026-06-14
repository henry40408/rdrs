# Sidebar Completed-Summary Count — Design

**Date:** 2026-06-14
**Branch:** `feat/sidebar-summary-count`
**Status:** Approved design, pending implementation plan

## Problem

The user often forgets they have already summarized entries. The sidebar surfaces
unread counts but gives no signal about summaries, and there is no "Summarized"
navigation entry at all (the `/entries/summarized` view is only reachable via the
`m` keyboard shortcut).

## Goal

Add a **"Summarized" navigation item** to the sidebar's first section (after
"Starred"), carrying a **count badge** of the user's completed summaries — both a
reminder ("you have N summaries") and a way to jump to `/entries/summarized`.

## Non-Goals

- Do **not** change the `/entries/summarized` view's filter. It currently lists
  entries that have *any* `entry_summary` row (any status). The badge counts
  `status='completed'` only, so the badge may differ slightly from the view's
  row count (failed / in-flight summaries). This minor divergence is accepted
  (user decision) — the badge's intent is "summaries I can actually read."
- No per-category summary counts (the count is global, like `total_unread` is a
  global badge on "Unread").
- No new frontend build tooling. The sidebar is pure CSR (`<rdrs-sidebar>` +
  the `#rdrs-sidebar-bootstrap` JSON); there is no SSR sidebar macro to update.

## Approach

Reuse the existing unread-count plumbing end to end. The count is a single
global integer `total_summarized` threaded through the same path as
`total_unread`.

### Data

- **New model fn** `entry_summary::count_completed(conn, user_id) -> AppResult<i64>`:
  `SELECT COUNT(*) FROM entry_summary WHERE user_id = ?1 AND status = 'completed'`.
  Index-covered by the existing `idx_entry_summary_user_status (user_id, status)`.
- **`CachedChrome`** (`src/services/sidebar_cache.rs`): add `total_summarized: i64`.
- **`ChromeData`** + **`SidebarResponse`** (`src/handlers/user.rs`): add
  `total_summarized: i64`.
- **`read_chrome_data`** (`src/handlers/user.rs`): in the SAME `read_user`
  closure that already fetches theme + categories + unread + has_feeds, add the
  `count_completed` query — one extra cheap query, no extra round-trip. Populate
  the cache + `ChromeData` with it.
- **`get_sidebar`** (`GET /api/sidebar`, the CSR background-revalidate endpoint):
  include `total_summarized` in its `SidebarResponse`.
- **Bootstrap** (`serialize_sidebar_for_script` in
  `src/handlers/pages/script_json.rs`): it does `serde_json::to_string(payload)`
  on the whole `SidebarResponse`, so the new field flows into
  `#rdrs-sidebar-bootstrap` automatically — **no change needed in this file**.

### UI (`static/js/components/rdrs-sidebar.js`)

- In the first `.sidebar-section`, after the "Starred" item, add a "Summarized"
  item mirroring the existing pattern:
  ```html
  <a href="/entries/summarized" class="sidebar-item${isActive('summarized')}" data-testid="nav-summarized">
      <span class="sidebar-item-icon">✨</span>
      <span>Summarized</span>
      <span class="sidebar-badge" id="summarized-count">${totalSummarized > 0 ? totalSummarized : ''}</span>
  </a>
  ```
  where `totalSummarized = data ? data.total_summarized : 0`.
- **Active state:** add `'summarized'` handling so the item highlights on the
  summarized view. Note the existing "All Entries" item currently treats
  `active === 'summarized'` as active; remove `'summarized'` from that item's
  active set so the new dedicated item owns the active state for that view.
- **Surgical badge patch:** in the same place the code updates `#unread-count`
  on a background revalidate, also update `#summarized-count` from
  `data.total_summarized` (show the number when > 0, else empty), so the badge
  advances without a full re-render.

### Cache invalidation

`total_summarized` lives in `sidebar_cache` (60 s TTL). Bust it when the count
changes so the badge updates promptly instead of lagging up to the TTL:

- `entry_summary::set_completed` path — the background summary worker that
  finishes a summary. After it persists `completed`, `state.sidebar_cache.bust(user_id)`.
- Summary **dismiss** (`entry_summary::delete`, the `data-summary-dismiss`
  endpoint) — bust after delete.

If a bust site is missed, the badge still self-heals within the 60 s TTL and on
the CSR's per-mount `/api/sidebar` revalidate. (Bulk deletions via retention are
not explicitly busted; they reconcile on TTL/revalidate — acceptable.)

## Testing

- **Rust unit:** `count_completed` returns the right number (0 when none; counts
  only `completed`, not pending/processing/failed; scoped per user).
- **Rust handler:** `GET /api/sidebar` response JSON includes `total_summarized`
  with the correct value after seeding a completed summary; the page bootstrap
  (`#rdrs-sidebar-bootstrap`) includes `total_summarized`.
- **Cache:** completing a summary (or dismissing one) leads to an updated count
  on the next read (bust verified — e.g. assert the count changes without
  waiting for TTL).
- **e2e:** the sidebar shows a "Summarized" item (`nav-summarized`); after
  seeding/creating a completed summary the `#summarized-count` badge shows the
  expected number; clicking it lands on `/entries/summarized`.

## Files Touched

- `src/models/entry_summary.rs` — `count_completed` + unit test.
- `src/services/sidebar_cache.rs` — `CachedChrome.total_summarized`.
- `src/handlers/user.rs` — `ChromeData` + `SidebarResponse` field;
  `read_chrome_data` query; `get_sidebar` includes it.
- `src/handlers/pages/script_json.rs` — no change (serializes the whole
  `SidebarResponse`; the new field flows through automatically).
- `static/js/components/rdrs-sidebar.js` — Summarized nav item + badge + active
  state + surgical patch.
- summary worker (`set_completed` caller) + dismiss handler — `sidebar_cache.bust`.
- `tests/handlers_test.rs` (+ model test) and `e2e/features/*.feature` — coverage.

## Risks & Mitigations

- **Stale badge** → cache bust on complete/dismiss + 60 s TTL + per-mount
  revalidate. Acceptable.
- **Active-state double-highlight** (All Entries + Summarized both active on the
  summarized view) → remove `'summarized'` from the All-Entries active set when
  adding the dedicated item.
- **Count/view divergence** (badge=completed, view=any-row) → accepted, documented
  in Non-Goals.
- **Extra query per chrome build** → folded into the existing cached `read_user`
  closure; index-covered; negligible.
